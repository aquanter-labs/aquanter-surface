use super::{
    AdapterError, Event, Exchange, MarketKind, StreamConfig, StreamKind, StreamTicksize, Venue,
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

    fn starts_at_bucket(&self) -> bool {
        self.bucket
            .zip(self.source_klines.first_key_value().map(|(time, _)| *time))
            .is_some_and(|(bucket, first)| bucket == first)
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
        venue: Venue,
        proxy: Option<&super::proxy::Proxy>,
    ) -> Result<(), AdapterError> {
        let client = reqwest::Client::new();
        match venue {
            Venue::Binance => {
                self.binance = Some(binance::BinanceHandle::new(client, proxy)?);
            }
            Venue::Bybit => {
                self.bybit = Some(bybit::BybitHandle::new(client, proxy)?);
            }
            Venue::Hyperliquid => {
                self.hyperliquid = Some(hyperliquid::HyperliquidHandle::new(client, proxy)?);
            }
            Venue::Okex => {
                self.okex = Some(okex::OkexHandle::new(client, proxy)?);
            }
            Venue::Mexc => {
                self.mexc = Some(mexc::MexcHandle::new(client, proxy)?);
            }
        }

        Ok(())
    }

    pub fn spawn_venues(
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
            if let Err(err) = out.spawn_venue(venue, proxy) {
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

        let handles = self.clone();
        let initialize = async move {
            let mut aggregators: HashMap<StreamKind, KlineAggregator> = HashMap::new();
            let now = UnixMs::now();

            for (source_stream, targets) in &targets_by_source {
                let Some((ticker_info, source_timeframe)) = source_stream.as_kline_stream() else {
                    continue;
                };
                let custom_targets = targets
                    .iter()
                    .filter_map(|target| {
                        let timeframe = target.as_kline_stream()?.1;
                        (timeframe != source_timeframe).then_some((*target, timeframe))
                    })
                    .collect::<Vec<_>>();
                let Some(from) = custom_targets
                    .iter()
                    .map(|(_, timeframe)| now.floor_to(*timeframe))
                    .min()
                else {
                    continue;
                };

                if let Ok(mut klines) = handles
                    .fetch_klines(ticker_info, source_timeframe, Some((from, now)))
                    .await
                {
                    klines.sort_unstable_by_key(|kline| kline.time);
                    for (target, timeframe) in custom_targets {
                        let aggregator = aggregators
                            .entry(target)
                            .or_insert_with(|| KlineAggregator::new(timeframe));
                        for kline in &klines {
                            aggregator.update(*kline);
                        }
                    }
                }
            }

            (aggregators, targets_by_source)
        };

        let target_scope = stream_scope.clone();
        let mut source_stream = Some(source_stream);
        stream::once(initialize)
            .flat_map(move |(mut aggregators, targets_by_source)| {
                let target_scope = target_scope.clone();
                source_stream.take().unwrap().flat_map(move |event| {
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
                                    || aggregator.starts_at_bucket())
                                .then_some(Event::KlineReceived(*target, kline))
                            })
                            .collect(),
                        Event::Connected(_) => vec![Event::Connected(target_scope.clone())],
                        Event::Disconnected(_, reason) => {
                            vec![Event::Disconnected(target_scope.clone(), reason)]
                        }
                        event => vec![event],
                    };
                    stream::iter(events)
                })
            })
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
        let exchange = ticker_info.ticker.exchange;
        let venue = exchange.venue();
        let source = exchange.kline_source_timeframe(timeframe).ok_or_else(|| {
            AdapterError::InvalidRequest(format!("Unsupported kline timeframe: {timeframe}"))
        })?;
        let range = kline_fetch_range(source, timeframe, range);

        let klines = match venue {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, source, range).await
            }
            Venue::Bybit => {
                let Some(handle) = self.bybit.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, source, range).await
            }
            Venue::Hyperliquid => {
                let Some(handle) = self.hyperliquid.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, source, range).await
            }
            Venue::Okex => {
                let Some(handle) = self.okex.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, source, range).await
            }
            Venue::Mexc => {
                let Some(handle) = self.mexc.as_ref() else {
                    return Err(Self::missing_venue_error(venue));
                };
                handle.fetch_klines(ticker_info, source, range).await
            }
        }?;

        Ok(if source == timeframe {
            klines
        } else {
            aggregate_klines(klines, timeframe)
        })
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
        data_path: Option<PathBuf>,
    ) -> Result<Vec<Trade>, AdapterError> {
        let exchange = ticker_info.ticker.exchange;

        match exchange.venue() {
            Venue::Binance => {
                let Some(handle) = self.binance.as_ref() else {
                    return Err(Self::missing_venue_error(exchange.venue()));
                };
                handle.fetch_trades(ticker_info, from_time, data_path).await
            }
            _ => Err(AdapterError::InvalidRequest(format!(
                "Trade fetch not available for {exchange}"
            ))),
        }
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
}

impl Default for AdapterHandles {
    fn default() -> Self {
        Self::spawn_venues(Venue::ALL, None)
    }
}

impl std::hash::Hash for AdapterHandles {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::any::TypeId::of::<Self>().hash(state);
    }
}
