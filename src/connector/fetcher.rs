use data::chart::kline::OpenInterestSeries;
use data::chart::kline::fetch_plan::supports_open_interest;
use exchange::adapter::{AdapterError, AdapterHandles, Exchange, KlinePage, StreamKind};
use exchange::{Kline, TickerInfo, Timeframe, Trade, UnixMs};
use iced::{
    Task,
    task::{Handle, Straw, sipper},
};
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use uuid::Uuid;

use crate::connector::client::{DataSources, ServerClient};

pub use data::TradeFetchMode;
pub use data::chart::kline::FetchRange;

static TRADE_FETCH_MODE: RwLock<TradeFetchMode> = RwLock::new(TradeFetchMode::Off);

/// Maximum trades to request per Arrow IPC call to server.
const ARROW_LIMIT: usize = 400_000;

/// Override the global trade-fetch mode at runtime.
pub fn set_trade_fetch_mode(mode: TradeFetchMode) {
    if let Ok(mut guard) = TRADE_FETCH_MODE.write() {
        *guard = mode;
    } else {
        log::error!("Trade fetch mode lock poisoned — resetting to Off");
    }
}

/// Return the current global trade-fetch mode.
pub fn trade_fetch_mode() -> TradeFetchMode {
    TRADE_FETCH_MODE
        .read()
        .map(|g| g.clone())
        .unwrap_or(TradeFetchMode::Off)
}

/// Returns `true` when a trade-fetch source (server or exchange API) is
/// active.
pub fn is_trade_fetch_enabled() -> bool {
    trade_fetch_mode() != TradeFetchMode::Off
}

/// Venues whose own API serves historical trades.
pub fn venue_has_trade_history(exchange: Exchange) -> bool {
    matches!(
        exchange.venue(),
        exchange::adapter::Venue::Binance
            | exchange::adapter::Venue::Bybit
            | exchange::adapter::Venue::Okex
    )
}

/// Whether a historical trade fetch for `exchange` can succeed right now.
pub fn has_trade_history(exchange: Exchange) -> bool {
    match trade_fetch_mode() {
        TradeFetchMode::Off => false,
        TradeFetchMode::Server => true,
        TradeFetchMode::Exchange => venue_has_trade_history(exchange),
    }
}

#[derive(Debug, Clone)]
pub enum FetchedData {
    Trades {
        batch: Vec<Trade>,
        /// Upper bound of the fetch gap — trades beyond this timestamp
        /// already exist on the chart and must be filtered out.
        until_time: UnixMs,
        /// Whether the paging task has finished producing batches.
        is_batches_done: bool,
        /// Whether the source returned any trade data for the request.
        has_data: bool,
        /// Optional request ID to mark completion on the chart's
        /// [`RequestHandler`] once the batch is processed.
        req_id: Option<Uuid>,
    },
    Klines {
        data: Vec<Kline>,
        req_id: Option<uuid::Uuid>,
        is_batches_done: bool,
        has_data: bool,
    },
    OpenInterest {
        series: OpenInterestSeries,
        req_id: Option<uuid::Uuid>,
    },
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum ReqError {
    #[error("Request overlaps with an existing request")]
    Overlaps,
    #[error("Source has no data for the requested range")]
    NoData,
    /// A previous attempt for this range failed and the retry cooldown
    /// has not yet elapsed.
    #[error("Previous request failed, retry not yet allowed")]
    Failed,
}

/// Lifecycle of a fetch request.
///
/// * `Failed` is retried after the cooldown (transient errors may resolve).
/// * `Completed` and `NoData` retain coverage evidence for non-profile inputs.
/// * Profile coverage lives in `ProfileKlineSource`, so completed profile
///   requests are removed.
#[derive(PartialEq, Clone, Debug)]
enum RequestStatus {
    Pending,
    /// Fetch succeeded and data was inserted.
    Completed,
    /// The source returned an empty result for this range.
    NoData,
    /// The fetch failed (network error, parse error, etc.).
    Failed(u64),
}

#[derive(Default)]
pub struct RequestHandler {
    requests: FxHashMap<Uuid, FetchRequest>,
}

impl RequestHandler {
    const RETRY_AFTER_MS: u64 = 30_000;

    pub fn add_request(&mut self, fetch: FetchRange) -> Result<Option<Uuid>, ReqError> {
        let request = FetchRequest::new(fetch);
        let id = Uuid::new_v4();

        if let Some((existing_id, status)) = self.requests.iter().find_map(|(k, v)| {
            if v.same_with(&request) {
                Some((*k, v.status.clone()))
            } else {
                None
            }
        }) {
            match status {
                RequestStatus::Completed => return Ok(None),
                RequestStatus::NoData => return Err(ReqError::NoData),
                RequestStatus::Pending if !is_history_fetch(&fetch) => {
                    return Err(ReqError::Overlaps);
                }
                RequestStatus::Failed(ts) if !is_history_fetch(&fetch) => {
                    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
                    if now_ms.saturating_sub(ts) > Self::RETRY_AFTER_MS {
                        if let Some(existing) = self.requests.get_mut(&existing_id) {
                            existing.status = RequestStatus::Pending;
                            existing.received_trades = 0;
                            existing.covered = None;
                        }
                        return Ok(Some(existing_id));
                    }
                    return Err(ReqError::Failed);
                }
                RequestStatus::Pending | RequestStatus::Failed(_) => {}
            }
        }

        if is_history_fetch(&fetch) {
            if self.requests.values().any(|existing| {
                existing.status == RequestStatus::Pending && existing.overlaps(&fetch)
            }) {
                return Err(ReqError::Overlaps);
            }

            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            let mut expired_failed = Vec::new();
            for (existing_id, existing) in &self.requests {
                let RequestStatus::Failed(failed_at) = existing.status else {
                    continue;
                };
                if !existing.overlaps(&fetch) {
                    continue;
                }
                if now_ms.saturating_sub(failed_at) <= Self::RETRY_AFTER_MS {
                    return Err(ReqError::Failed);
                }
                expired_failed.push(*existing_id);
            }
            for existing_id in expired_failed {
                self.requests.remove(&existing_id);
            }
        }

        self.requests.insert(id, request);
        Ok(Some(id))
    }

    pub fn is_pending(&self, id: Uuid) -> bool {
        self.requests
            .get(&id)
            .is_some_and(|request| request.status == RequestStatus::Pending)
    }

    pub fn is_profile_request(&self, id: Uuid) -> bool {
        self.requests.get(&id).is_some_and(|request| {
            request.status == RequestStatus::Pending
                && matches!(request.fetch_type, FetchRange::ProfileKline { .. })
        })
    }

    pub fn request_ids(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.requests.keys().copied()
    }

    pub fn has_pending_trade_fetch(&self) -> bool {
        self.requests.values().any(|request| {
            request.status == RequestStatus::Pending
                && matches!(request.fetch_type, FetchRange::Trades(..))
        })
    }

    pub fn has_pending_profile_fetch(&self) -> bool {
        self.requests.values().any(|request| {
            request.status == RequestStatus::Pending
                && matches!(request.fetch_type, FetchRange::ProfileKline { .. })
        })
    }

    pub fn pending_ranges(&self) -> impl Iterator<Item = FetchRange> + '_ {
        self.requests
            .values()
            .filter(|request| request.status == RequestStatus::Pending)
            .map(|request| request.fetch_type)
    }

    /// Trades received so far and the fraction of the requested time range they cover.
    pub fn pending_trade_progress(&self) -> Option<(usize, Option<f32>)> {
        self.requests.values().find_map(|request| {
            let FetchRange::Trades(from, to) = request.fetch_type else {
                return None;
            };
            (request.status == RequestStatus::Pending).then(|| {
                let span = to.0.saturating_sub(from.0);
                let progress = request.covered.filter(|_| span > 0).map(|(lo, hi)| {
                    (hi.min(to.0).saturating_sub(lo.max(from.0)) as f32 / span as f32)
                        .clamp(0.0, 1.0)
                });
                (request.received_trades, progress)
            })
        })
    }

    pub fn record_trade_batch(&mut self, id: Uuid, count: usize, span: Option<(u64, u64)>) -> bool {
        let Some(request) = self.requests.get_mut(&id) else {
            return false;
        };

        if request.status != RequestStatus::Pending
            || !matches!(request.fetch_type, FetchRange::Trades(..))
        {
            return false;
        }

        request.received_trades = request.received_trades.saturating_add(count);
        if let Some((lo, hi)) = span {
            // ponytail: assumes batches arrive contiguously from one end of the range
            request.covered = Some(match request.covered {
                Some((a, b)) => (a.min(lo), b.max(hi)),
                None => (lo, hi),
            });
        }
        true
    }

    pub fn mark_completed(&mut self, id: Uuid) -> bool {
        if self.is_profile_request(id) {
            return self.mark_profile_completed(id).is_some();
        }

        let Some(request) = self.requests.get_mut(&id) else {
            return false;
        };
        if request.status != RequestStatus::Pending {
            return false;
        }

        request.status = RequestStatus::Completed;
        true
    }

    pub fn mark_profile_completed(&mut self, id: Uuid) -> Option<FetchRange> {
        let request = self.requests.get(&id)?;
        if request.status != RequestStatus::Pending
            || !matches!(request.fetch_type, FetchRange::ProfileKline { .. })
        {
            return None;
        }
        self.requests.remove(&id).map(|request| request.fetch_type)
    }

    /// Mark a request as completed with no data — the source returned an
    /// empty result.  The range will never be retried.
    pub fn mark_no_data(&mut self, id: Uuid) -> bool {
        if self.is_profile_request(id) {
            self.requests.remove(&id);
            return true;
        }

        let Some(request) = self.requests.get_mut(&id) else {
            return false;
        };
        if request.status != RequestStatus::Pending {
            return false;
        }
        request.status = RequestStatus::NoData;
        true
    }

    pub fn mark_failed(&mut self, id: Uuid) -> bool {
        if let Some(request) = self.requests.get_mut(&id) {
            if request.status != RequestStatus::Pending {
                return false;
            }
            let timestamp = chrono::Utc::now().timestamp_millis() as u64;
            request.status = RequestStatus::Failed(timestamp);
            true
        } else {
            false
        }
    }

    pub fn clear_open_interest_requests(&mut self) -> Vec<Uuid> {
        let removed = self
            .requests
            .iter()
            .filter(|(_, request)| matches!(request.fetch_type, FetchRange::OpenInterest { .. }))
            .map(|(id, _)| *id)
            .collect();
        self.requests
            .retain(|_, request| !matches!(request.fetch_type, FetchRange::OpenInterest { .. }));
        removed
    }

    pub fn clear_pending_profile_requests(&mut self) -> Vec<Uuid> {
        let removed = self
            .requests
            .iter()
            .filter(|(_, request)| {
                matches!(request.fetch_type, FetchRange::ProfileKline { .. })
                    && request.status == RequestStatus::Pending
            })
            .map(|(id, _)| *id)
            .collect();
        self.requests.retain(|_, request| {
            !matches!(request.fetch_type, FetchRange::ProfileKline { .. })
                || request.status != RequestStatus::Pending
        });
        removed
    }

    pub fn clear_pending_trade_requests(&mut self) -> Vec<Uuid> {
        let removed = self
            .requests
            .iter()
            .filter(|(_, request)| {
                matches!(request.fetch_type, FetchRange::Trades(..))
                    && request.status == RequestStatus::Pending
            })
            .map(|(id, _)| *id)
            .collect();
        self.requests.retain(|_, request| {
            !matches!(request.fetch_type, FetchRange::Trades(..))
                || request.status != RequestStatus::Pending
        });
        removed
    }

    pub fn reset_for_basis_change(&mut self) {
        self.requests.clear();
    }
}

#[derive(PartialEq, Debug)]
struct FetchRequest {
    fetch_type: FetchRange,
    status: RequestStatus,
    received_trades: usize,
    /// Earliest/latest trade time received, for progress display.
    covered: Option<(u64, u64)>,
}

impl FetchRequest {
    fn new(fetch_type: FetchRange) -> Self {
        FetchRequest {
            fetch_type,
            status: RequestStatus::Pending,
            received_trades: 0,
            covered: None,
        }
    }

    fn same_with(&self, other: &FetchRequest) -> bool {
        self.same_with_range(&other.fetch_type)
    }

    /// Check whether the stored [`FetchRange`] matches a given range.
    fn same_with_range(&self, range: &FetchRange) -> bool {
        match (&self.fetch_type, range) {
            (FetchRange::Kline(s1, e1), FetchRange::Kline(s2, e2)) => e1 == e2 && s1 == s2,
            (
                FetchRange::ProfileKline {
                    timeframe: t1,
                    from: s1,
                    to: e1,
                },
                FetchRange::ProfileKline {
                    timeframe: t2,
                    from: s2,
                    to: e2,
                },
            ) => t1 == t2 && e1 == e2 && s1 == s2,
            (
                FetchRange::OpenInterest { from: s1, to: e1 },
                FetchRange::OpenInterest { from: s2, to: e2 },
            ) => e1 == e2 && s1 == s2,
            (FetchRange::Trades(s1, e1), FetchRange::Trades(s2, e2)) => e1 == e2 && s1 == s2,
            _ => false,
        }
    }

    fn overlaps(&self, range: &FetchRange) -> bool {
        match (&self.fetch_type, range) {
            (FetchRange::Kline(s1, e1), FetchRange::Kline(s2, e2)) => s1 <= e2 && s2 <= e1,
            (
                FetchRange::OpenInterest { from: s1, to: e1 },
                FetchRange::OpenInterest { from: s2, to: e2 },
            ) => s1 <= e2 && s2 <= e1,
            (
                FetchRange::ProfileKline {
                    timeframe: t1,
                    from: s1,
                    to: e1,
                },
                FetchRange::ProfileKline {
                    timeframe: t2,
                    from: s2,
                    to: e2,
                },
            ) => t1 == t2 && s1 < e2 && s2 < e1,
            _ => false,
        }
    }
}

fn is_history_fetch(range: &FetchRange) -> bool {
    matches!(
        range,
        FetchRange::Kline(..) | FetchRange::ProfileKline { .. } | FetchRange::OpenInterest { .. }
    )
}

pub struct FetchSpec {
    pub req_id: uuid::Uuid,
    pub fetch: FetchRange,
    pub stream: Option<StreamKind>,
}

impl From<(uuid::Uuid, FetchRange, Option<StreamKind>)> for FetchSpec {
    fn from(t: (uuid::Uuid, FetchRange, Option<StreamKind>)) -> Self {
        FetchSpec {
            req_id: t.0,
            fetch: t.1,
            stream: t.2,
        }
    }
}

impl std::fmt::Debug for FetchSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchSpec")
            .field("req_id", &self.req_id)
            .field("fetch", &self.fetch)
            .field("stream", &self.stream)
            .finish()
    }
}

impl Clone for FetchSpec {
    fn clone(&self) -> Self {
        FetchSpec {
            req_id: self.req_id,
            fetch: self.fetch,
            stream: self.stream,
        }
    }
}

#[derive(Debug, Clone)]
pub enum FetchUpdate {
    Data {
        pane_id: Uuid,
        stream: StreamKind,
        data: FetchedData,
    },
    Error {
        pane_id: Uuid,
        error: String,
        req_id: Option<Uuid>,
    },
}

pub fn request_fetch(
    sources: &DataSources,
    pane_id: Uuid,
    ready_streams: &[StreamKind],
    req_id: Uuid,
    fetch: FetchRange,
    stream: Option<StreamKind>,
    on_fetch_handle: &mut impl FnMut(Uuid, Handle),
) -> Task<FetchUpdate> {
    let handles = sources.exchange.clone();

    match fetch {
        FetchRange::Kline(from, to) | FetchRange::ProfileKline { from, to, .. } => {
            let kline_stream = if let Some(s) = stream {
                Some((s, pane_id))
            } else {
                ready_streams.iter().find_map(|stream| {
                    if let StreamKind::Kline { .. } = stream {
                        Some((*stream, pane_id))
                    } else {
                        None
                    }
                })
            };

            if let Some((
                StreamKind::Kline {
                    ticker_info,
                    timeframe,
                },
                pane_uid,
            )) = kline_stream
            {
                let now = UnixMs::now();
                let (timeframe, from, to) = match fetch {
                    FetchRange::ProfileKline { timeframe, .. } => {
                        (timeframe, from, to.min(now.floor_to(timeframe)))
                    }
                    _ => (
                        timeframe,
                        from.floor_to(timeframe),
                        to.floor_to(timeframe)
                            .offset_by_timeframe(timeframe, 1)
                            .min(now.floor_to(timeframe)),
                    ),
                };
                let (task, handle) = Task::sip(
                    fetch_klines_paged(handles, ticker_info, timeframe, from, to),
                    move |batch| FetchUpdate::Data {
                        pane_id: pane_uid,
                        stream: StreamKind::Kline {
                            ticker_info,
                            timeframe,
                        },
                        data: FetchedData::Klines {
                            data: batch,
                            req_id: Some(req_id),
                            is_batches_done: false,
                            has_data: true,
                        },
                    },
                    move |result| match result {
                        Ok(has_data) => FetchUpdate::Data {
                            pane_id: pane_uid,
                            stream: StreamKind::Kline {
                                ticker_info,
                                timeframe,
                            },
                            data: FetchedData::Klines {
                                data: Vec::new(),
                                req_id: Some(req_id),
                                is_batches_done: true,
                                has_data,
                            },
                        },
                        Err(err) => {
                            log::error!("Kline fetch failed: {err}");
                            FetchUpdate::Error {
                                pane_id: pane_uid,
                                error: err.ui_message(),
                                req_id: Some(req_id),
                            }
                        }
                    },
                )
                .abortable();

                on_fetch_handle(req_id, handle.abort_on_drop());
                return task;
            }
        }
        FetchRange::OpenInterest { from, to } => {
            let kline_stream = if let Some(s) = stream {
                Some((s, pane_id))
            } else {
                ready_streams.iter().find_map(|stream| {
                    if let StreamKind::Kline { .. } = stream {
                        Some((*stream, pane_id))
                    } else {
                        None
                    }
                })
            };

            if let Some((stream, pane_uid)) = kline_stream {
                return open_interest_fetch_task(
                    handles,
                    pane_uid,
                    stream,
                    Some(req_id),
                    Some((from, to)),
                    on_fetch_handle,
                );
            }
        }
        FetchRange::Trades(from_time, to_time) => {
            let trade_stream = stream
                .filter(|stream| matches!(stream, StreamKind::Trades { .. }))
                .or_else(|| {
                    ready_streams.iter().find_map(|stream| {
                        if let StreamKind::Trades { .. } = stream {
                            Some(*stream)
                        } else {
                            None
                        }
                    })
                });

            if let Some(trade_stream @ StreamKind::Trades { ticker_info }) = trade_stream {
                let has_venue_history = venue_has_trade_history(ticker_info.exchange());
                let mode = trade_fetch_mode();
                let server = sources.server.clone();
                let data_path = data::data_path(Some(&format!(
                    "market_data/{}/",
                    format!("{:?}", ticker_info.exchange().venue()).to_lowercase()
                )));

                if let Some(ref client) = server {
                    log::debug!(
                        "Trade fetch: using server at {} ({})",
                        client.base_url(),
                        ticker_info.exchange()
                    );
                } else if mode == TradeFetchMode::Server {
                    log::error!(
                        "Server mode selected but server URL is invalid, check Network Manager settings"
                    );
                    return Task::done(FetchUpdate::Error {
                        pane_id,
                        error: "Server mode selected but the server URL is invalid.".to_string(),
                        req_id: Some(req_id),
                    });
                } else if has_venue_history {
                    log::debug!(
                        "Trade fetch: using direct exchange API for {}",
                        ticker_info.exchange()
                    );
                } else {
                    log::warn!(
                        "Trade fetch via exchange API is only supported for Binance, Bybit and OKX, got {}",
                        ticker_info.exchange()
                    );
                    return Task::done(FetchUpdate::Error {
                        pane_id,
                        error: format!(
                            "Trade fetch via exchange API is only supported for Binance, Bybit and OKX, got {}",
                            ticker_info.exchange()
                        ),
                        req_id: Some(req_id),
                    });
                }

                let (task, handle) = Task::sip(
                    fetch_trades_paged(server, handles, ticker_info, from_time, to_time, data_path),
                    move |batch| {
                        let data = FetchedData::Trades {
                            batch,
                            req_id: Some(req_id),
                            until_time: to_time,
                            is_batches_done: false,
                            has_data: true,
                        };

                        FetchUpdate::Data {
                            pane_id,
                            data,
                            stream: trade_stream,
                        }
                    },
                    move |result| match result {
                        Ok(true) => FetchUpdate::Data {
                            pane_id,
                            data: FetchedData::Trades {
                                batch: Vec::new(),
                                req_id: Some(req_id),
                                until_time: to_time,
                                is_batches_done: true,
                                has_data: true,
                            },
                            stream: trade_stream,
                        },
                        Ok(false) => {
                            // Source returned no data for this range. Produce
                            // an empty batch so the dashboard calls mark_no_data.
                            FetchUpdate::Data {
                                pane_id,
                                data: FetchedData::Trades {
                                    batch: Vec::new(),
                                    req_id: Some(req_id),
                                    until_time: to_time,
                                    is_batches_done: true,
                                    has_data: false,
                                },
                                stream: trade_stream,
                            }
                        }
                        Err(err) => {
                            log::error!("Trade fetch failed: {err}");
                            FetchUpdate::Error {
                                pane_id,
                                error: err.ui_message(),
                                req_id: Some(req_id),
                            }
                        }
                    },
                )
                .abortable();

                on_fetch_handle(req_id, handle.abort_on_drop());

                return task;
            }
        }
    }

    Task::none()
}

pub fn request_fetch_many(
    sources: &DataSources,
    pane_id: Uuid,
    ready_streams: &[StreamKind],
    reqs: impl IntoIterator<Item = (Uuid, FetchRange, Option<StreamKind>)>,
    mut on_fetch_handle: impl FnMut(Uuid, Handle),
) -> Task<FetchUpdate> {
    Task::batch(reqs.into_iter().map(|(req_id, fetch, stream)| {
        request_fetch(
            sources,
            pane_id,
            ready_streams,
            req_id,
            fetch,
            stream,
            &mut on_fetch_handle,
        )
    }))
}

pub fn open_interest_fetch_task(
    handles: AdapterHandles,
    pane_id: Uuid,
    stream: StreamKind,
    req_id: Option<Uuid>,
    range: Option<(UnixMs, UnixMs)>,
    on_fetch_handle: &mut impl FnMut(Uuid, Handle),
) -> Task<FetchUpdate> {
    let fetch_task = match stream {
        StreamKind::Kline {
            ticker_info,
            timeframe,
        } => {
            if !supports_open_interest(ticker_info, timeframe) {
                return Task::done(FetchUpdate::Error {
                    pane_id,
                    error: format!(
                        "Open Interest is not available for {}",
                        ticker_info.exchange()
                    ),
                    req_id,
                });
            }

            let fetch = async move {
                handles
                    .fetch_open_interest(ticker_info, timeframe, range)
                    .await
                    .map(|data| {
                        OpenInterestSeries::from_points(
                            data.into_iter().map(|point| (point.time, point.value)),
                        )
                    })
            };

            Task::perform(
                iced::futures::TryFutureExt::map_err(fetch, move |err| {
                    log::error!("Open Interest fetch failed: {err}");
                    err.ui_message()
                }),
                move |result| match result {
                    Ok(batch) => {
                        let data = FetchedData::OpenInterest {
                            series: batch,
                            req_id,
                        };
                        FetchUpdate::Data {
                            pane_id,
                            data,
                            stream,
                        }
                    }
                    Err(err) => FetchUpdate::Error {
                        pane_id,
                        error: err,
                        req_id,
                    },
                },
            )
        }
        _ => Task::none(),
    };

    let (task, handle) = fetch_task.abortable();
    if let Some(req_id) = req_id {
        on_fetch_handle(req_id, handle.abort_on_drop());
    }
    task
}

fn fetch_klines_paged(
    handles: AdapterHandles,
    ticker_info: TickerInfo,
    timeframe: Timeframe,
    from_time: UnixMs,
    to_time: UnixMs,
) -> impl Straw<bool, Vec<Kline>, AdapterError> {
    sipper(async move |mut progress| {
        let mut cursor = from_time;
        let mut had_data = false;

        while cursor < to_time {
            let page = handles
                .fetch_klines_page(ticker_info, timeframe, cursor, to_time)
                .await?;
            let next_cursor = advance_kline_page(cursor, to_time, &page)?;
            if !page.data.is_empty() {
                had_data = true;
                progress.send(page.data).await;
            }
            cursor = next_cursor;
        }

        Ok(had_data)
    })
}

fn advance_kline_page(
    cursor: UnixMs,
    end: UnixMs,
    page: &KlinePage,
) -> Result<UnixMs, AdapterError> {
    if page.next_from <= cursor {
        return Err(AdapterError::ParseError(
            "Kline paging range did not advance".to_string(),
        ));
    }
    if page.next_from > end {
        return Err(AdapterError::ParseError(
            "Kline paging cursor exceeded requested end".to_string(),
        ));
    }
    Ok(page.next_from)
}

/// Fetch trades newest-first so the latest price fills before older history.
///
/// Returns `Ok(true)` when at least one trade was received, `Ok(false)`
/// when the source confirmed the range has no data.
pub fn fetch_trades_paged(
    server: Option<ServerClient>,
    handles: AdapterHandles,
    ticker_info: TickerInfo,
    from_time: UnixMs,
    to_time: UnixMs,
    data_path: PathBuf,
) -> impl Straw<bool, Vec<Trade>, AdapterError> {
    sipper(async move |mut progress| {
        // Windows are half-open `[start, end)` and walk newest → oldest; each
        // window is paged oldest → newest until complete before moving on.
        let mut end = to_time.saturating_add(1);
        let mut span = TRADE_WINDOW_MIN_MS;
        let mut had_data = false;

        while end > from_time {
            let today = UnixMs::now().floor_to(Timeframe::D1);
            let start = if server.is_some() {
                end.saturating_sub(TRADE_WINDOW_MIN_MS).max(from_time)
            } else {
                direct_trade_window_start(from_time, end, today, span)
            };

            if let Some(ref client) = server {
                let mut cursor = start;
                while cursor < end {
                    let mut parsed = client
                        .fetch_trades_arrow(ticker_info, cursor, end, ARROW_LIMIT)
                        .await?;
                    let next = server_trade_page(
                        &mut parsed.trades,
                        cursor,
                        end,
                        parsed.raw_row_count,
                        parsed.last_ts,
                    )?;
                    if !parsed.trades.is_empty() {
                        had_data = true;
                        progress.send(parsed.trades).await;
                    }
                    let Some(next) = next else {
                        break;
                    };
                    cursor = next;
                }
            } else {
                let batch = handles
                    .fetch_trades(ticker_info, start, end, Some(data_path.clone()))
                    .await?;
                span = if batch.len() < 1000 {
                    (span * 2).min(TRADE_WINDOW_MAX_MS)
                } else {
                    TRADE_WINDOW_MIN_MS
                };
                if !batch.is_empty() {
                    had_data = true;
                    progress.send(batch).await;
                }
            }

            if start >= end {
                return Err(malformed_trade_source(end));
            }
            end = start;
        }

        Ok(had_data)
    })
}

const TRADE_WINDOW_MIN_MS: u64 = 60_000;
const TRADE_WINDOW_MAX_MS: u64 = 60 * 60_000;

/// Start of the next direct-fetch window ending at `end`. Today's data is
/// fetched in short windows so the latest trades arrive first; earlier days
/// are fetched as whole UTC days, matching Binance's daily archives.
fn direct_trade_window_start(from: UnixMs, end: UnixMs, today: UnixMs, span: u64) -> UnixMs {
    let start = if end > today {
        end.saturating_sub(span).max(today)
    } else {
        end.saturating_sub(1).floor_to(Timeframe::D1)
    };
    start.max(from)
}

/// Keeps the complete part of a server page and returns where the next page
/// starts. The server has no trade cursor, so a full page may stop inside its
/// last millisecond: that millisecond is dropped here and refetched whole.
fn server_trade_page(
    trades: &mut Vec<Trade>,
    cursor: UnixMs,
    end: UnixMs,
    raw_row_count: usize,
    last_ts: Option<UnixMs>,
) -> Result<Option<UnixMs>, AdapterError> {
    trades.retain(|trade| trade.time < end);
    if raw_row_count < ARROW_LIMIT {
        return Ok(None);
    }
    let boundary = last_ts.unwrap_or(cursor);
    if boundary <= cursor {
        log::error!(
            "trade page at {cursor:?} is filled by a single millisecond; the server needs a trade-id cursor to page it"
        );
        return Err(AdapterError::ParseError(
            "Trade page limit reached within one millisecond".to_string(),
        ));
    }
    trades.retain(|trade| trade.time < boundary);
    Ok(Some(boundary))
}

fn malformed_trade_source(at: UnixMs) -> AdapterError {
    log::error!(
        "trade paging did not advance at {at:?}; aborting to avoid an infinite loop (source may be malformed)"
    );
    AdapterError::ParseError("Source may be malformed. Check logs for details.".to_string())
}

#[cfg(test)]
mod trade_window_tests {
    use super::{ARROW_LIMIT, TRADE_WINDOW_MIN_MS, direct_trade_window_start, server_trade_page};
    use exchange::{Trade, UnixMs, unit::Price, unit::Qty};

    fn trades(times: &[u64]) -> Vec<Trade> {
        times
            .iter()
            .map(|&time| Trade {
                time: UnixMs::new(time),
                is_sell: false,
                price: Price::from_f64(1.0),
                qty: Qty::ZERO,
            })
            .collect()
    }

    #[test]
    fn full_server_page_refetches_its_last_millisecond() {
        let mut page = trades(&[10, 11, 12, 12]);
        let next = server_trade_page(
            &mut page,
            UnixMs::new(10),
            UnixMs::new(100),
            ARROW_LIMIT,
            Some(UnixMs::new(12)),
        )
        .unwrap();
        assert_eq!(next, Some(UnixMs::new(12)));
        assert!(page.iter().all(|trade| trade.time < UnixMs::new(12)));
        assert_eq!(page.len(), 2);

        let mut last = trades(&[12, 12, 13, 100]);
        assert_eq!(
            server_trade_page(
                &mut last,
                UnixMs::new(12),
                UnixMs::new(100),
                4,
                Some(UnixMs::new(100))
            )
            .unwrap(),
            None
        );
        assert_eq!(last.len(), 3);

        let mut stuck = trades(&[12, 12]);
        assert!(
            server_trade_page(
                &mut stuck,
                UnixMs::new(12),
                UnixMs::new(100),
                ARROW_LIMIT,
                Some(UnixMs::new(12))
            )
            .is_err()
        );
    }

    const DAY: u64 = 86_400_000;

    #[test]
    fn direct_windows_tile_the_range_without_gaps() {
        let today = UnixMs::new(10 * DAY);
        let from = UnixMs::new(8 * DAY + 5);
        let mut end = UnixMs::new(10 * DAY + 150_000);
        let mut starts = Vec::new();
        while end > from {
            let start = direct_trade_window_start(from, end, today, TRADE_WINDOW_MIN_MS);
            assert!(start < end);
            starts.push(start.as_u64());
            end = start;
        }
        assert_eq!(
            starts,
            vec![
                10 * DAY + 90_000,
                10 * DAY + 30_000,
                10 * DAY,
                9 * DAY,
                8 * DAY + 5
            ]
        );
    }
}
