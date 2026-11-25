use super::{
    AdapterError, Event, Exchange, KlinePage, MarketKind, StreamConfig, StreamKind, StreamTicksize,
    Venue,
    hub::{binance, bybit, hyperliquid, mexc, okex},
};
use crate::{
    Kline, OpenInterest, TickMultiplier, Ticker, TickerInfo, TickerStats, Timeframe, Trade, UnixMs,
    Volume,
};

use futures::{StreamExt, stream, stream::BoxStream};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

// Keep topics per websocket conservative across venues
// allow up to 100 tickers per websocket stream
pub const MAX_TRADE_TICKERS_PER_STREAM: usize = 100;
pub const MAX_KLINE_STREAMS_PER_STREAM: usize = 100;

struct KlineAggregator {
    timeframe: Timeframe,
    bucket: Option<UnixMs>,
    source_klines: BTreeMap<UnixMs, Kline>,
}

impl KlineAggregator {
    fn new(timeframe: Timeframe) -> Self {
        Self {
            timeframe,
            bucket: None,
            source_klines: BTreeMap::new(),
        }
    }

    fn update(&mut self, kline: Kline) -> Option<Kline> {
        let bucket = kline.time.floor_to(self.timeframe);
        if self.bucket.is_some_and(|current| bucket < current) {
            return None;
        }
        if self.bucket != Some(bucket) {
            self.bucket = Some(bucket);
            self.source_klines.clear();
        }
        self.source_klines.insert(kline.time, kline);

        let mut klines = self.source_klines.values();
        let first = *klines.next()?;
        let mut result = Kline {
            time: bucket,
            ..first
        };
        for next in klines {
            result.high = result.high.max(next.high);
            result.low = result.low.min(next.low);
            result.close = next.close;
            result.volume = add_volume(result.volume, next.volume);
        }
        Some(result)
    }

    fn covers_sources(&self, source: Timeframe) -> bool {
        let starts_at_bucket = self
            .bucket
            .zip(self.source_klines.first_key_value().map(|(time, _)| *time))
            .is_some_and(|(bucket, first)| bucket == first);
        let mut expected = self.bucket;
        starts_at_bucket
            && self.source_klines.keys().all(|time| {
                let contiguous = expected == Some(*time);
                expected = Some(time.saturating_add(source.to_milliseconds()));
                contiguous
            })
    }
}

fn add_volume(left: Volume, right: Volume) -> Volume {
    match (left, right) {
        (Volume::BuySell(left_buy, left_sell), Volume::BuySell(right_buy, right_sell)) => {
            Volume::BuySell(left_buy + right_buy, left_sell + right_sell)
        }
        (left, right) => Volume::TotalOnly(left.total() + right.total()),
    }
}

fn aggregate_klines(mut klines: Vec<Kline>, timeframe: Timeframe) -> Vec<Kline> {
    klines.sort_unstable_by_key(|kline| kline.time);
    let mut aggregator = KlineAggregator::new(timeframe);
    let mut aggregated = BTreeMap::new();
    for kline in klines {
        if let Some(kline) = aggregator.update(kline) {
            aggregated.insert(kline.time, kline);
        }
    }
    aggregated.into_values().collect()
}

fn kline_fetch_range(
    source: Timeframe,
    target: Timeframe,
    range: Option<(UnixMs, UnixMs)>,
) -> Option<(UnixMs, UnixMs)> {
    range.or_else(|| {
        (source != target).then(|| {
            let now = UnixMs::now();
            (now.saturating_sub(400 * target.to_milliseconds()), now)
        })
    })
}

fn open_interest_page_end(from: UnixMs, to: UnixMs, source: Timeframe) -> UnixMs {
    // Stay below every supported venue's page cap, even with missing samples.
    to.min(from.saturating_add(100 * source.to_milliseconds() - 1))
}

async fn fetch_open_interest_range<F, Fut>(
    from: UnixMs,
    to: UnixMs,
    source: Timeframe,
    mut fetch: F,
) -> Result<Vec<OpenInterest>, AdapterError>
where
    F: FnMut((UnixMs, UnixMs)) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<OpenInterest>, AdapterError>>,
{
    let mut cursor = from;
    let mut points = Vec::new();
    while cursor <= to {
        let end = open_interest_page_end(cursor, to, source);
        points.extend(fetch((cursor, end)).await?);
        if end == to {
            break;
        }
        cursor = end.saturating_add(1);
    }
    points.retain(|point| point.time >= from && point.time <= to);
    points.sort_unstable_by_key(|point| point.time);
    points.dedup_by_key(|point| point.time);
    Ok(points)
}

#[derive(Clone)]
pub struct AdapterHandles {
    binance: Option<binance::BinanceHandle>,
    bybit: Option<bybit::BybitHandle>,
    hyperliquid: Option<hyperliquid::HyperliquidHandle>,
    okex: Option<okex::OkexHandle>,
    mexc: Option<mexc::MexcHandle>,
}

impl AdapterHandles {
    pub fn spawn_venue(
        &mut self,
        client: &reqwest::Client,
        venue: Venue,
        proxy: Option<&super::proxy::Proxy>,
    ) -> Result<(), AdapterError> {
        match venue {
            Venue::Binance => {
                self.binance = Some(binance::BinanceHandle::new(client.clone(), proxy)?);
            }
            Venue::Bybit => {
                self.bybit = Some(bybit::BybitHandle::new(client.clone(), proxy)?);
            }
            Venue::Hyperliquid => {
                self.hyperliquid =
                    Some(hyperliquid::HyperliquidHandle::new(client.clone(), proxy)?);
            }
            Venue::Okex => {
                self.okex = Some(okex::OkexHandle::new(client.clone(), proxy)?);
            }
            Venue::Mexc => {
                self.mexc = Some(mexc::MexcHandle::new(client.clone(), proxy)?);
            }
        }

        Ok(())
    }

    pub fn spawn_venues(
        client: &reqwest::Client,
        venues: impl IntoIterator<Item = Venue>,
        proxy: Option<&super::proxy::Proxy>,
    ) -> Self {
        let mut out = Self {
            binance: None,
            bybit: None,
            hyperliquid: None,
            okex: None,
            mexc: None,
        };

        for venue in venues {
            if let Err(err) = out.spawn_venue(client, venue, proxy) {
                log::error!("Failed to spawn {venue} adapter: {err}");
            }
        }

        out
    }

    fn missing_venue_stream(
        exchange: Exchange,
        streams: Arc<[StreamKind]>,
    ) -> BoxStream<'static, Event> {
        let err = format!("Adapter unavailable for {}", exchange.venue());
        stream::once(async move { Event::Disconnected(streams, err) }).boxed()
    }

    fn missing_venue_error(venue: Venue) -> AdapterError {
        let err = format!("Adapter unavailable for venue {venue}");
        AdapterError::unavailable(venue, err)
    }

    fn depth_scope(config: &StreamConfig<TickerInfo>) -> Arc<[StreamKind]> {
        let ticker_info = config.id;
        let depth_aggr = if config.exchange.is_depth_client_aggr() {
            StreamTicksize::Client
        } else {
            StreamTicksize::ServerSide(config.tick_mltp.unwrap_or(TickMultiplier(1)))
        };

        Arc::from(
            vec![StreamKind::Depth {
                ticker_info,
                depth_aggr,
                push_freq: config.push_freq,
            }]
            .into_boxed_slice(),
        )
    }

    fn trade_scope(config: &StreamConfig<Vec<TickerInfo>>) -> Arc<[StreamKind]> {
        Arc::from(
            config
                .id
                .iter()
                .map(|ticker_info| StreamKind::Trades {
                    ticker_info: *ticker_info,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        )
    }

    fn liquidation_scope(config: &StreamConfig<Vec<TickerInfo>>) -> Arc<[StreamKind]> {
        Arc::from(
            config
                .id
                .iter()
                .map(|ticker_info| StreamKind::Liquidations {
                    ticker_info: *ticker_info,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        )
    }

    fn kline_scope(config: &StreamConfig<Vec<(TickerInfo, Timeframe)>>) -> Arc<[StreamKind]> {
        Arc::from(
            config
                .id
                .iter()
                .map(|(ticker_info, timeframe)| StreamKind::Kline {
                    ticker_info: *ticker_info,
                    timeframe: *timeframe,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        )
    }

    pub fn kline_stream(
        &self,
        config: &StreamConfig<Vec<(TickerInfo, Timeframe)>>,
    ) -> BoxStream<'static, Event> {
        let stream_scope = Self::kline_scope(config);
        let mut targets_by_source: HashMap<StreamKind, Vec<StreamKind>> = HashMap::new();
        let mut streams = Vec::new();
        let mut seen_sources = HashSet::new();

        for &(ticker_info, target) in &config.id {
            let Some(source) = config.exchange.kline_source_timeframe(target) else {
                let reason = format!("Unsupported kline timeframe: {target}");
                return stream::once(async move { Event::Disconnected(stream_scope, reason) })
                    .boxed();
            };
            let source_stream = StreamKind::Kline {
                ticker_info,
                timeframe: source,
            };
            let target_stream = StreamKind::Kline {
                ticker_info,
                timeframe: target,
            };
            targets_by_source
                .entry(source_stream)
                .or_default()
                .push(target_stream);
            if seen_sources.insert(source_stream) {
                streams.push((ticker_info, source));
            }
        }

        let market_kind = config.exchange.market_type();

        let missing_venue_stream =
            || Self::missing_venue_stream(config.exchange, stream_scope.clone());

        let source_stream = match config.exchange.venue() {
            Venue::Binance => self
                .binance
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_kline_stream(streams, market_kind).boxed()
                }),
            Venue::Bybit => self
                .bybit
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_kline_stream(streams, market_kind).boxed()
                }),
            Venue::Hyperliquid => self
                .hyperliquid
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_kline_stream(streams, market_kind).boxed()
                }),
            Venue::Okex => self
                .okex
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_kline_stream(streams, market_kind).boxed()
                }),
            Venue::Mexc => self
                .mexc
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_kline_stream(streams, market_kind).boxed()
                }),
        };

        let state = (
            source_stream,
            HashMap::<StreamKind, KlineAggregator>::new(),
            self.clone(),
            targets_by_source,
            stream_scope,
        );
        stream::unfold(state, |state| async move {
            let (mut source_stream, mut aggregators, handles, targets_by_source, target_scope) =
                state;
            let event = source_stream.next().await?;
            if matches!(event, Event::Connected(_)) {
                aggregators.clear();
                let now = UnixMs::now();
                for (source_stream, targets) in &targets_by_source {
                    let (ticker_info, source) = source_stream.as_kline_stream().unwrap();
                    let custom_targets = targets
                        .iter()
                        .filter_map(|target| {
                            let timeframe = target.as_kline_stream()?.1;
                            (timeframe != source).then_some((*target, timeframe))
                        })
                        .collect::<Vec<_>>();
                    let Some(from) = custom_targets
                        .iter()
                        .map(|(_, target)| now.floor_to(*target))
                        .min()
                    else {
                        continue;
                    };
                    match handles
                        .fetch_native_klines_range(ticker_info, source, from, now)
                        .await
                    {
                        Ok(klines) => {
                            for (target, timeframe) in custom_targets {
                                let aggregator = aggregators
                                    .entry(target)
                                    .or_insert_with(|| KlineAggregator::new(timeframe));
                                for kline in &klines {
                                    aggregator.update(*kline);
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!("Failed to seed custom klines for {ticker_info:?}: {error}")
                        }
                    }
                }
            }
            let events = match event {
                Event::KlineReceived(source, kline) => targets_by_source
                    .get(&source)
                    .into_iter()
                    .flatten()
                    .filter_map(|target| {
                        let source_timeframe = source.as_kline_stream()?.1;
                        let target_timeframe = target.as_kline_stream()?.1;
                        let aggregator = aggregators
                            .entry(*target)
                            .or_insert_with(|| KlineAggregator::new(target_timeframe));
                        let kline = aggregator.update(kline)?;
                        (source_timeframe == target_timeframe
                            || aggregator.covers_sources(source_timeframe))
                        .then_some(Event::KlineReceived(*target, kline))
                    })
                    .collect(),
                Event::Connected(_) => vec![Event::Connected(target_scope.clone())],
                Event::Disconnected(_, reason) => {
                    vec![Event::Disconnected(target_scope.clone(), reason)]
                }
                event => vec![event],
            };
            Some((
                events,
                (
                    source_stream,
                    aggregators,
                    handles,
                    targets_by_source,
                    target_scope,
                ),
            ))
        })
        .flat_map(stream::iter)
        .boxed()
    }

    pub fn trade_stream(
        &self,
        config: &StreamConfig<Vec<TickerInfo>>,
    ) -> BoxStream<'static, Event> {
        let stream_scope = Self::trade_scope(config);
        let streams = config.id.clone();
        let market_kind = config.exchange.market_type();

        let missing_venue_stream =
            || Self::missing_venue_stream(config.exchange, stream_scope.clone());

        match config.exchange.venue() {
            Venue::Binance => self
                .binance
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_trade_stream(streams, market_kind).boxed()
                }),
            Venue::Bybit => self
                .bybit
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_trade_stream(streams, market_kind).boxed()
                }),
            Venue::Hyperliquid => self
                .hyperliquid
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_trade_stream(streams, market_kind).boxed()
                }),
            Venue::Okex => self
                .okex
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_trade_stream(streams, market_kind).boxed()
                }),
            Venue::Mexc => self
                .mexc
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle.connect_trade_stream(streams, market_kind).boxed()
                }),
        }
    }

    pub fn liquidation_stream(
        &self,
        config: &StreamConfig<Vec<TickerInfo>>,
    ) -> BoxStream<'static, Event> {
        let stream_scope = Self::liquidation_scope(config);
        let missing_venue_stream = || Self::missing_venue_stream(config.exchange, stream_scope);
        if config.exchange.venue() != Venue::Binance
            || config.exchange.market_type() == MarketKind::Spot
        {
            return missing_venue_stream();
        }
        self.binance
            .clone()
            .map_or_else(missing_venue_stream, |handle| {
                handle
                    .connect_liquidation_stream(config.id.clone(), config.exchange.market_type())
                    .boxed()
            })
    }

    pub fn depth_stream(&self, config: &StreamConfig<TickerInfo>) -> BoxStream<'static, Event> {
        let stream_scope = Self::depth_scope(config);
        let ticker_info = config.id;
        let push_freq = config.push_freq;
        let depth_aggr = if config.exchange.is_depth_client_aggr() {
            StreamTicksize::Client
        } else {
            StreamTicksize::ServerSide(config.tick_mltp.unwrap_or(TickMultiplier(1)))
        };

        let missing_venue_stream =
            || Self::missing_venue_stream(config.exchange, stream_scope.clone());

        match config.exchange.venue() {
            Venue::Binance => self
                .binance
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle
                        .connect_depth_stream(ticker_info, depth_aggr, push_freq)
                        .boxed()
                }),
            Venue::Bybit => self
                .bybit
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle
                        .connect_depth_stream(ticker_info, depth_aggr, push_freq)
                        .boxed()
                }),
            Venue::Hyperliquid => {
                self.hyperliquid
                    .clone()
                    .map_or_else(missing_venue_stream, |handle| {
                        handle
                            .connect_depth_stream(ticker_info, depth_aggr, push_freq)
                            .boxed()
                    })
            }
            Venue::Okex => self
                .okex
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle
                        .connect_depth_stream(ticker_info, depth_aggr, push_freq)
                        .boxed()
                }),
            Venue::Mexc => self
                .mexc
                .clone()
                .map_or_else(missing_venue_stream, |handle| {
                    handle
                        .connect_depth_stream(ticker_info, depth_aggr, push_freq)
                        .boxed()
                }),
        }
    }

    /// Returns a map of tickers to their [`TickerInfo`].
    /// If metadata for a ticker can't be fetched/parsed expectedly, it will still be included in the map as `None`.
    ///
    /// `Binance`, `Bybit`, and `Hyperliquid` are fetched per market, while
    /// `Okex` and `Mexc` handle market branching internally due to combined perps endpoints.
    pub async fn fetch_ticker_metadata(
        &self,
        venue: Venue,
        markets: &[MarketKind],
    ) -> Result<HashMap<Ticker, Option<TickerInfo>>, AdapterError> {
        match venue {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(
                        handle
                            .fetch_ticker_metadata(binance::BinanceMarketScope::metadata(*market))
                            .await?,
                    );
                }
                Ok(out)
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(handle.fetch_ticker_metadata(*market).await?);
                }
                Ok(out)
            }
            Venue::Hyperliquid => {
                let Some(handle) = self.hyperliquid.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(handle.fetch_ticker_metadata(*market).await?);
                }
                Ok(out)
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_ticker_metadata(markets.to_vec()).await
            }
            Venue::Mexc => {
                let Some(handle) = self.mexc.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_ticker_metadata(mexc::MexcMarketScope::metadata(markets))
                    .await
            }
        }
    }

    /// Returns a map of tickers to their [`TickerStats`].
    ///
    /// `Binance`, `Bybit`, and `Hyperliquid` are fetched per market, while
    /// `Okex` and `Mexc` handle market branching internally due to combined perps endpoints.
    pub async fn fetch_ticker_stats(
        &self,
        venue: Venue,
        markets: &[MarketKind],
        contract_sizes: Option<HashMap<Ticker, crate::unit::ContractSize>>,
    ) -> Result<HashMap<Ticker, TickerStats>, AdapterError> {
        match venue {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(
                        handle
                            .fetch_ticker_stats(binance::BinanceMarketScope::stats(
                                *market,
                                contract_sizes.clone(),
                            ))
                            .await?,
                    );
                }
                Ok(out)
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(handle.fetch_ticker_stats(*market).await?);
                }
                Ok(out)
            }
            Venue::Hyperliquid => {
                let Some(handle) = self.hyperliquid.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };

                let mut out = HashMap::default();
                for market in markets {
                    out.extend(handle.fetch_ticker_stats(*market).await?);
                }
                Ok(out)
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_ticker_stats(markets.to_vec()).await
            }
            Venue::Mexc => {
                let Some(handle) = self.mexc.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_ticker_stats(mexc::MexcMarketScope::stats(markets, contract_sizes))
                    .await
            }
        }
    }

    pub async fn fetch_klines(
        &self,
        ticker_info: TickerInfo,
        timeframe: Timeframe,
        range: Option<(UnixMs, UnixMs)>,
    ) -> Result<Vec<Kline>, AdapterError> {
        let source = ticker_info
            .exchange()
            .kline_source_timeframe(timeframe)
            .ok_or_else(|| {
                AdapterError::InvalidRequest(format!("Unsupported kline timeframe: {timeframe}"))
            })?;
        if source != timeframe {
            let (from, to) = kline_fetch_range(source, timeframe, range).unwrap();
            let from = from.floor_to(timeframe);
            let mut cursor = from;
            let mut klines = Vec::new();
            while cursor < to {
                let page = self
                    .fetch_klines_page(ticker_info, timeframe, cursor, to)
                    .await?;
                if page.next_from <= cursor || page.next_from > to {
                    return Err(AdapterError::ParseError(
                        "Kline paging did not advance within range".into(),
                    ));
                }
                cursor = page.next_from;
                klines.extend(page.data);
            }
            return Ok(klines);
        }
        let venue = ticker_info.ticker.exchange.venue();

        match venue {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, timeframe, range).await
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, timeframe, range).await
            }
            Venue::Hyperliquid => {
                let Some(handle) = self.hyperliquid.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, timeframe, range).await
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, timeframe, range).await
            }
            Venue::Mexc => {
                let Some(handle) = self.mexc.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, timeframe, range).await
            }
        }
    }

    pub async fn fetch_klines_page(
        &self,
        ticker_info: TickerInfo,
        timeframe: Timeframe,
        from: UnixMs,
        to: UnixMs,
    ) -> Result<KlinePage, AdapterError> {
        let source = ticker_info
            .exchange()
            .kline_source_timeframe(timeframe)
            .ok_or_else(|| {
                AdapterError::InvalidRequest(format!("Unsupported kline timeframe: {timeframe}"))
            })?;
        if source == timeframe {
            return self
                .fetch_native_klines_page(ticker_info, source, from, to)
                .await;
        }
        if from >= to {
            return Err(AdapterError::InvalidRequest(
                "Kline range must be nonempty".into(),
            ));
        }
        // Keep source pages together until a complete target boundary is reached.
        let end = to.min(
            from.floor_to(timeframe)
                .saturating_add(400 * timeframe.to_milliseconds()),
        );
        let data = self
            .fetch_native_klines_range(ticker_info, source, from.floor_to(timeframe), end)
            .await?;
        Ok(KlinePage::new(aggregate_klines(data, timeframe), from, end))
    }

    async fn fetch_native_klines_range(
        &self,
        ticker_info: TickerInfo,
        source: Timeframe,
        from: UnixMs,
        end: UnixMs,
    ) -> Result<Vec<Kline>, AdapterError> {
        let mut cursor = from;
        let mut data = Vec::new();
        while cursor < end {
            let page = self
                .fetch_native_klines_page(ticker_info, source, cursor, end)
                .await?;
            if page.next_from <= cursor || page.next_from > end {
                return Err(AdapterError::ParseError(
                    "Kline paging did not advance within range".into(),
                ));
            }
            cursor = page.next_from;
            data.extend(page.data);
        }
        data.sort_unstable_by_key(|kline| kline.time);
        Ok(data)
    }

    async fn fetch_native_klines_page(
        &self,
        ticker_info: TickerInfo,
        timeframe: Timeframe,
        from: UnixMs,
        to: UnixMs,
    ) -> Result<KlinePage, AdapterError> {
        let venue = ticker_info.ticker.exchange.venue();

        match venue {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_klines_page(ticker_info, timeframe, from, to)
                    .await
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_klines_page(ticker_info, timeframe, from, to)
                    .await
            }
            Venue::Hyperliquid => {
                let Some(handle) = self.hyperliquid.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_klines_page(ticker_info, timeframe, from, to)
                    .await
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_klines_page(ticker_info, timeframe, from, to)
                    .await
            }
            Venue::Mexc => {
                let Some(handle) = self.mexc.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle
                    .fetch_klines_page(ticker_info, timeframe, from, to)
                    .await
            }
        }
    }

    pub async fn fetch_open_interest(
        &self,
        ticker_info: TickerInfo,
        timeframe: Timeframe,
        range: Option<(UnixMs, UnixMs)>,
    ) -> Result<Vec<OpenInterest>, AdapterError> {
        let exchange = ticker_info.ticker.exchange;
        let source = exchange
            .open_interest_source_timeframe(timeframe)
            .ok_or_else(|| {
                AdapterError::InvalidRequest(format!(
                    "Open interest data not available for {exchange} at {timeframe}"
                ))
            })?;

        let now = UnixMs::now();
        let (from, to) =
            range.unwrap_or_else(|| (now.saturating_sub(400 * timeframe.to_milliseconds()), now));
        if from > to {
            return Err(AdapterError::InvalidRequest(
                "Open interest range is reversed".into(),
            ));
        }
        let mut cursor = from.floor_to(source);
        if exchange.venue() == Venue::Binance {
            cursor = cursor.max(now.saturating_sub(30 * 24 * 60 * 60 * 1000));
        }
        let mut points = fetch_open_interest_range(cursor, to, source, |range| {
            self.fetch_native_open_interest(ticker_info, source, Some(range))
        })
        .await?;
        points.retain(|point| point.time >= from && point.time <= to);
        Ok(points)
    }

    async fn fetch_native_open_interest(
        &self,
        ticker_info: TickerInfo,
        source: Timeframe,
        range: Option<(UnixMs, UnixMs)>,
    ) -> Result<Vec<OpenInterest>, AdapterError> {
        let exchange = ticker_info.exchange();

        match exchange {
            Exchange::BinanceLinear | Exchange::BinanceInverse => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle.fetch_open_interest(ticker_info, source, range).await
            }
            Exchange::BybitLinear | Exchange::BybitInverse => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle.fetch_open_interest(ticker_info, source, range).await
            }
            Exchange::OkexLinear | Exchange::OkexInverse => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle.fetch_open_interest(ticker_info, source, range).await
            }
            _ => Err(AdapterError::InvalidRequest(format!(
                "Open interest data not available for {exchange}"
            ))),
        }
    }

    pub async fn fetch_trades(
        &self,
        ticker_info: TickerInfo,
        from_time: UnixMs,
        until: UnixMs,
        data_path: Option<PathBuf>,
    ) -> Result<Vec<Trade>, AdapterError> {
        let exchange = ticker_info.ticker.exchange;

        match exchange.venue() {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle
                    .fetch_trades(ticker_info, from_time, until, data_path)
                    .await
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle
                    .fetch_trades(ticker_info, from_time, until, data_path)
                    .await
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle
                    .fetch_trades(ticker_info, from_time, until, data_path)
                    .await
            }
            _ => Err(AdapterError::InvalidRequest(format!(
                "Trade fetch not available for {exchange}"
            ))),
        }
    }
}

impl std::hash::Hash for AdapterHandles {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::any::TypeId::of::<Self>().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{Price, Qty};

    fn kline(time: u64, open: f64, high: f64, low: f64, close: f64, volume: Volume) -> Kline {
        Kline {
            time: UnixMs::new(time),
            open: Price::from_f64(open),
            high: Price::from_f64(high),
            low: Price::from_f64(low),
            close: Price::from_f64(close),
            volume,
        }
    }

    #[test]
    fn aggregates_native_updates_into_custom_timeframe() {
        let mut aggregator = KlineAggregator::new(Timeframe::Custom(10));
        aggregator.update(kline(
            0,
            10.0,
            12.0,
            9.0,
            11.0,
            Volume::BuySell(Qty::from_f64(1.0), Qty::from_f64(2.0)),
        ));
        aggregator.update(kline(
            300_000,
            11.0,
            15.0,
            10.0,
            14.0,
            Volume::BuySell(Qty::from_f64(3.0), Qty::from_f64(4.0)),
        ));
        let result = aggregator
            .update(kline(
                300_000,
                11.0,
                16.0,
                8.0,
                13.0,
                Volume::BuySell(Qty::from_f64(5.0), Qty::from_f64(7.0)),
            ))
            .unwrap();

        assert_eq!(result.time, UnixMs::ZERO);
        assert_eq!(result.open, Price::from_f64(10.0));
        assert_eq!(result.high, Price::from_f64(16.0));
        assert_eq!(result.low, Price::from_f64(8.0));
        assert_eq!(result.close, Price::from_f64(13.0));
        assert_eq!(
            result.volume.buy_sell(),
            Some((Qty::from_f64(6.0), Qty::from_f64(9.0)))
        );
        assert_eq!(
            Exchange::BinanceSpot.kline_source_timeframe(Timeframe::Custom(10)),
            Some(Timeframe::M5)
        );
        assert_eq!(
            Exchange::BinanceLinear.open_interest_source_timeframe(Timeframe::Custom(10)),
            Some(Timeframe::M5)
        );
        assert_eq!(
            Exchange::BinanceLinear.open_interest_source_timeframe(Timeframe::Custom(11)),
            None
        );
        assert!(aggregator.covers_sources(Timeframe::M5));
        assert!(
            aggregator
                .update(kline(0, 1.0, 1.0, 1.0, 1.0, Volume::empty_total()))
                .is_some()
        );
        aggregator.update(kline(1_500_000, 1.0, 1.0, 1.0, 1.0, Volume::empty_total()));
        assert!(!aggregator.covers_sources(Timeframe::M5));
        assert!(
            aggregator
                .update(kline(0, 1.0, 1.0, 1.0, 1.0, Volume::empty_total()))
                .is_none()
        );
    }

    #[test]
    fn historical_custom_klines_cross_native_page_boundaries_without_losing_volume() {
        let source = (0..2000)
            .map(|index| {
                kline(
                    index * 300_000,
                    index as f64,
                    index as f64 + 2.0,
                    index as f64 - 1.0,
                    index as f64 + 1.0,
                    Volume::BuySell(Qty::from_f64(1.0), Qty::from_f64(2.0)),
                )
            })
            .collect::<Vec<_>>();
        let targets = aggregate_klines(source, Timeframe::Custom(10));
        assert_eq!(targets.len(), 1000);
        assert_eq!(targets[499].time.as_u64(), 499 * 600_000);
        assert_eq!(targets[500].open, Price::from_f64(1000.0));
        assert!(
            targets
                .iter()
                .all(|kline| kline.volume.total() == Qty::from_f64(6.0))
        );
    }

    #[tokio::test]
    async fn unavailable_custom_and_liquidation_streams_report_target_scopes() {
        let handles = AdapterHandles {
            binance: None,
            bybit: None,
            hyperliquid: None,
            okex: None,
            mexc: None,
        };
        let info = TickerInfo::new(
            Ticker::new("BTCUSDT", Exchange::BinanceLinear),
            0.1,
            0.001,
            None,
        );
        let mut klines = handles.kline_stream(&StreamConfig::new(
            vec![(info, Timeframe::Custom(10))],
            info.exchange(),
            None,
            crate::PushFrequency::ServerDefault,
        ));
        let Some(Event::Disconnected(scope, _)) = klines.next().await else {
            panic!("expected disconnect");
        };
        assert_eq!(
            scope.as_ref(),
            &[StreamKind::Kline {
                ticker_info: info,
                timeframe: Timeframe::Custom(10)
            }]
        );
        let mut liquidations = handles.liquidation_stream(&StreamConfig::new(
            vec![info],
            info.exchange(),
            None,
            crate::PushFrequency::ServerDefault,
        ));
        let Some(Event::Disconnected(scope, _)) = liquidations.next().await else {
            panic!("expected disconnect");
        };
        assert_eq!(
            scope.as_ref(),
            &[StreamKind::Liquidations { ticker_info: info }]
        );
        let unique = super::super::UniqueStreams::from(scope.iter());
        assert_eq!(
            unique.liquidation_streams(Some(info.exchange())),
            vec![info]
        );
        assert_eq!(
            unique.combined()[info.exchange()]
                .as_ref()
                .unwrap()
                .liquidation,
            vec![info]
        );
    }

    #[test]
    fn custom_timeframe_initial_fetch_requests_400_target_candles() {
        let (from, to) = kline_fetch_range(Timeframe::M5, Timeframe::Custom(10), None).unwrap();

        assert_eq!(
            to.as_u64() - from.as_u64(),
            400 * Timeframe::Custom(10).to_milliseconds()
        );
        assert_eq!(
            kline_fetch_range(Timeframe::M15, Timeframe::M15, None),
            None
        );
    }

    #[tokio::test]
    async fn custom_ten_minute_oi_pages_through_missing_ranges_and_preserves_freshness() {
        let target = Timeframe::Custom(10);
        let source = Exchange::BinanceLinear
            .open_interest_source_timeframe(target)
            .unwrap();
        let step = source.to_milliseconds();
        let end = UnixMs::new(400 * target.to_milliseconds());
        let mut ranges = Vec::new();
        let points = fetch_open_interest_range(UnixMs::ZERO, end, source, |(from, to)| {
            ranges.push((from, to));
            let data = if from == UnixMs::new(100 * step) {
                Vec::new()
            } else {
                (from.as_u64()..=to.as_u64())
                    .step_by(step as usize)
                    .map(|time| OpenInterest {
                        time: time.into(),
                        value: time as f64,
                    })
                    .collect()
            };
            std::future::ready(Ok(data))
        })
        .await
        .unwrap();

        assert_eq!(ranges.len(), 9);
        assert!(
            ranges
                .windows(2)
                .all(|pair| pair[0].1.saturating_add(1) == pair[1].0)
        );
        assert_eq!(ranges.last().unwrap().1, end);
        assert_eq!(points.len(), 701);
        assert_eq!(points.last().unwrap().time, end);
        assert!(
            points
                .iter()
                .any(|point| point.time == UnixMs::new(200 * step))
        );
        assert!(
            !points
                .iter()
                .any(|point| point.time == UnixMs::new(100 * step))
        );

        let mut requested = None;
        let backfill = fetch_open_interest_range(
            UnixMs::new(100 * step),
            UnixMs::new(200 * step - 1),
            source,
            |range| {
                requested = Some(range);
                std::future::ready(Ok(vec![OpenInterest {
                    time: range.0,
                    value: 42.0,
                }]))
            },
        )
        .await
        .unwrap();
        assert_eq!(
            requested,
            Some((UnixMs::new(100 * step), UnixMs::new(200 * step - 1)))
        );
        assert_eq!(backfill[0].time, UnixMs::new(100 * step));
    }

    #[test]
    fn open_interest_limits_include_endpoints_and_never_become_zero() {
        for cap in [200, 500] {
            assert_eq!(
                super::super::open_interest_limit(0, 0, Timeframe::M5, cap),
                1
            );
            assert_eq!(
                super::super::open_interest_limit(0, 1, Timeframe::M5, cap),
                1
            );
            assert_eq!(
                super::super::open_interest_limit(0, 300_000, Timeframe::M5, cap),
                2
            );
            assert_eq!(
                super::super::open_interest_limit(0, u64::MAX, Timeframe::M5, cap),
                cap
            );
        }
    }
}
