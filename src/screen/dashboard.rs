pub mod pane;
pub mod panel;
pub mod sidebar;
pub mod tickers_table;

pub use sidebar::Sidebar;

use super::DashboardError;
use crate::{
    chart,
    connector::{
        ResolvedStream,
        fetcher::{self, FetchedData, InfoKind},
    },
    screen::dashboard::tickers_table::TickersTable,
    style,
    widget::toast::Toast,
    window::{self, Window},
};
use data::{
    UserTimezone,
    layout::{WindowSpec, pane::ContentKind, pane::VisualConfig},
    stream::PersistStreamKind,
};
use exchange::{
    Kline, Liquidation, PushFrequency, StreamPairKind, TickerInfo, Trade, UnixMs,
    adapter::{
        AdapterHandles, MAX_KLINE_STREAMS_PER_STREAM, MAX_TRADE_TICKERS_PER_STREAM, StreamConfig,
        StreamKind, StreamTicksize, UniqueStreams,
    },
    depth::Depth,
};

use iced::{
    Element, Length, Subscription, Task, Vector,
    widget::{
        PaneGrid, center, container,
        pane_grid::{self, Configuration},
    },
};
use std::{collections::HashMap, time::Instant, vec};

fn market_label(market: exchange::adapter::MarketKind) -> &'static str {
    match market {
        exchange::adapter::MarketKind::Spot => "SPOT",
        exchange::adapter::MarketKind::LinearPerps => "PERP",
        exchange::adapter::MarketKind::InversePerps => "INVERSE PERP",
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Pane(window::Id, pane::Message),
    ChangePaneStatus(uuid::Uuid, pane::Status),
    SavePopoutSpecs(HashMap<window::Id, WindowSpec>),
    ErrorOccurred(Option<uuid::Uuid>, DashboardError),
    FetchFailed {
        pane_id: uuid::Uuid,
        req_id: Option<uuid::Uuid>,
        error: String,
    },
    Notification(Toast),
    DistributeFetchedData {
        layout_id: uuid::Uuid,
        pane_id: uuid::Uuid,
        stream: StreamKind,
        data: FetchedData,
    },
    ResolveStreams(uuid::Uuid, Vec<PersistStreamKind>),
    Trading(uuid::Uuid, crate::trading::Message),
    TradingLevelDragged(uuid::Uuid, crate::trading::TradingLineId, f64, bool),
    RequestPalette,
}

pub struct Dashboard {
    pub panes: pane_grid::State<pane::State>,
    pub focus: Option<(window::Id, pane_grid::Pane)>,
    pub popout: HashMap<window::Id, (pane_grid::State<pane::State>, WindowSpec)>,
    pub streams: UniqueStreams,
    layout_id: uuid::Uuid,
}

fn set_fetch_status(current: &mut pane::Status, next: pane::Status) {
    let trades_are_loading = matches!(
        &*current,
        pane::Status::Loading(InfoKind::FetchingTrades(_))
    );
    let next_is_trade_status = matches!(&next, pane::Status::Loading(InfoKind::FetchingTrades(_)));

    if !trades_are_loading || next_is_trade_status {
        *current = next;
    }
}

impl Default for Dashboard {
    fn default() -> Self {
        Self {
            panes: pane_grid::State::with_configuration(Self::default_pane_config()),
            focus: None,
            streams: UniqueStreams::default(),
            popout: HashMap::new(),
            layout_id: uuid::Uuid::new_v4(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Ai(crate::ai::Message),
    Notification(Toast),
    DistributeFetchedData {
        layout_id: uuid::Uuid,
        pane_id: uuid::Uuid,
        data: FetchedData,
        stream: StreamKind,
    },
    ResolveStreams {
        pane_id: uuid::Uuid,
        streams: Vec<PersistStreamKind>,
    },
    RequestPalette,
}

impl Dashboard {
    fn default_pane_config() -> Configuration<pane::State> {
        Configuration::Split {
            axis: pane_grid::Axis::Vertical,
            ratio: 0.8,
            a: Box::new(Configuration::Split {
                axis: pane_grid::Axis::Horizontal,
                ratio: 0.4,
                a: Box::new(Configuration::Split {
                    axis: pane_grid::Axis::Vertical,
                    ratio: 0.5,
                    a: Box::new(Configuration::Pane(pane::State::default())),
                    b: Box::new(Configuration::Pane(pane::State::default())),
                }),
                b: Box::new(Configuration::Split {
                    axis: pane_grid::Axis::Vertical,
                    ratio: 0.5,
                    a: Box::new(Configuration::Pane(pane::State::default())),
                    b: Box::new(Configuration::Pane(pane::State::default())),
                }),
            }),
            b: Box::new(Configuration::Pane(pane::State::default())),
        }
    }

    pub fn from_config(
        panes: Configuration<pane::State>,
        popout_windows: Vec<(Configuration<pane::State>, WindowSpec)>,
        layout_id: uuid::Uuid,
    ) -> Self {
        let panes = pane_grid::State::with_configuration(panes);

        let mut popout = HashMap::new();

        for (pane, specs) in popout_windows {
            popout.insert(
                window::Id::unique(),
                (pane_grid::State::with_configuration(pane), specs),
            );
        }

        Self {
            panes,
            focus: None,
            streams: UniqueStreams::default(),
            popout,
            layout_id,
        }
    }

    pub fn load_layout(&mut self, main_window: window::Id) -> Task<Message> {
        let mut open_popouts_tasks: Vec<Task<Message>> = vec![];
        let mut new_popout = Vec::new();
        let mut keys_to_remove = Vec::new();

        for (old_window_id, (_, specs)) in &self.popout {
            keys_to_remove.push((*old_window_id, *specs));
        }

        // remove keys and open new windows
        for (old_window_id, window_spec) in keys_to_remove {
            let (window, task) = window::open(window::Settings {
                position: window::Position::Specific(window_spec.position()),
                size: window_spec.size(),
                exit_on_close_request: false,
                ..window::settings()
            });

            open_popouts_tasks.push(task.then(|_| Task::none()));

            if let Some((removed_pane, specs)) = self.popout.remove(&old_window_id) {
                new_popout.push((window, (removed_pane, specs)));
            }
        }

        // assign new windows to old panes
        for (window, (pane, specs)) in new_popout {
            self.popout.insert(window, (pane, specs));
        }

        Task::batch(open_popouts_tasks).chain(self.refresh_streams(main_window))
    }

    pub fn update(
        &mut self,
        handles: &AdapterHandles,
        message: Message,
        main_window: &Window,
        layout_id: &uuid::Uuid,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::SavePopoutSpecs(specs) => {
                for (window_id, new_spec) in specs {
                    if let Some((_, spec)) = self.popout.get_mut(&window_id) {
                        *spec = new_spec;
                    }
                }
            }
            Message::ErrorOccurred(pane_id, err) => match pane_id {
                Some(id) => {
                    if let Some(state) = self.get_mut_pane_state_by_uuid(main_window.id, id) {
                        state.status = pane::Status::Ready;
                        state.notifications.push(Toast::error(err.to_string()));
                    }
                }
                _ => {
                    return (
                        Task::done(Message::Notification(Toast::error(err.to_string()))),
                        None,
                    );
                }
            },
            Message::FetchFailed {
                pane_id,
                req_id,
                error,
            } => {
                if let Some(state) = self.get_mut_pane_state_by_uuid(main_window.id, pane_id) {
                    state.status = pane::Status::Ready;
                    if let pane::Content::Kline {
                        chart: Some(chart), ..
                    } = &mut state.content
                    {
                        chart.fail_request(req_id, error.clone());
                    }
                    state
                        .notifications
                        .push(Toast::error(DashboardError::Fetch(error, req_id).to_string()));
                }
            }
            Message::Pane(window, message) => match message {
                pane::Message::PaneClicked(pane) => {
                    self.focus = Some((window, pane));
                }
                pane::Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                    self.panes.resize(split, ratio);
                }
                pane::Message::PaneDragged(event) => {
                    if let pane_grid::DragEvent::Dropped { pane, target } = event {
                        self.panes.drop(pane, target);
                    }
                }
                pane::Message::SplitPane(axis, pane) => {
                    let focus_pane = if let Some((new_pane, _)) =
                        self.panes.split(axis, pane, pane::State::new())
                    {
                        Some(new_pane)
                    } else {
                        None
                    };

                    if let Some(focus_pane) = focus_pane {
                        self.focus = Some((window, focus_pane));
                    }
                }
                pane::Message::ClosePane(pane) => {
                    if let Some((_, sibling)) = self.panes.close(pane) {
                        self.focus = Some((window, sibling));
                    }
                }
                pane::Message::MaximizePane(pane) => {
                    self.panes.maximize(pane);
                }
                pane::Message::Restore => {
                    self.panes.restore();
                }
                pane::Message::ReplacePane(pane) => {
                    if let Some(pane) = self.panes.get_mut(pane) {
                        *pane = pane::State::new();
                    }

                    return (self.refresh_streams(main_window.id), None);
                }
                pane::Message::VisualConfigChanged(pane, cfg, to_sync) => {
                    if to_sync {
                        if let Some(state) = self.get_pane(main_window.id, window, pane) {
                            let studies_cfg = state.content.studies();
                            let clusters_cfg = state.content.clusters();

                            self.iter_all_panes_mut(main_window.id)
                                .for_each(|(_, _, state)| {
                                    let should_apply = match state.settings.visual_config {
                                        Some(ref current_cfg) => {
                                            std::mem::discriminant(current_cfg)
                                                == std::mem::discriminant(&cfg)
                                        }
                                        None => matches!(
                                            (&cfg, &state.content),
                                            (
                                                data::layout::pane::VisualConfig::Kline(_),
                                                pane::Content::Kline { .. }
                                            ) | (
                                                data::layout::pane::VisualConfig::Heatmap(_),
                                                pane::Content::Heatmap { .. }
                                                    | pane::Content::ShaderHeatmap { .. }
                                            ) | (
                                                data::layout::pane::VisualConfig::TimeAndSales(_),
                                                pane::Content::TimeAndSales(_)
                                            ) | (
                                                data::layout::pane::VisualConfig::Comparison(_),
                                                pane::Content::Comparison(_)
                                            )
                                        ),
                                    };

                                    if should_apply {
                                        state.settings.visual_config = Some(cfg.clone());
                                        state.content.change_visual_config(cfg.clone());

                                        state.apply_synced_settings(&studies_cfg, &clusters_cfg);
                                    }
                                });
                        }
                    } else if let Some(state) = self.get_mut_pane(main_window.id, window, pane) {
                        state.settings.visual_config = Some(cfg.clone());
                        state.content.change_visual_config(cfg);
                    }
                }
                pane::Message::SwitchLinkGroup(pane, group) => {
                    if group.is_none() {
                        if let Some(state) = self.get_mut_pane(main_window.id, window, pane) {
                            state.link_group = None;
                        }
                        return (Task::none(), None);
                    }

                    let maybe_ticker_info = self
                        .iter_all_panes(main_window.id)
                        .filter(|(w, p, _)| !(*w == window && *p == pane))
                        .find_map(|(_, _, other_state)| {
                            if other_state.link_group == group {
                                other_state.stream_pair()
                            } else {
                                None
                            }
                        });

                    if let Some(state) = self.get_mut_pane(main_window.id, window, pane) {
                        state.link_group = group;
                        state.modal = None;

                        if let Some(ticker_info) = maybe_ticker_info
                            && state.stream_pair() != Some(ticker_info)
                        {
                            let pane_id = state.unique_id();
                            let content_kind = state.content.kind();

                            let streams =
                                state.set_content_and_streams(vec![ticker_info], content_kind);
                            self.streams.extend(streams.iter());

                            for stream in &streams {
                                if let StreamKind::Kline { .. } = stream {
                                    return (
                                        fetcher::kline_fetch_task(
                                            handles.clone(),
                                            *layout_id,
                                            pane_id,
                                            *stream,
                                            None,
                                            None,
                                        )
                                        .map(Message::from),
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
                pane::Message::Popout => {
                    return (self.popout_pane(main_window), None);
                }
                pane::Message::Merge => {
                    return (self.merge_pane(main_window), None);
                }
                pane::Message::PaneEvent(pane, local) => {
                    if let Some(state) = self.get_mut_pane(main_window.id, window, pane) {
                        let Some(effect) = state.update(local) else {
                            return (Task::none(), None);
                        };

                        let task = match effect {
                            pane::Effect::RefreshStreams => self.refresh_streams(main_window.id),
                            pane::Effect::RequestFetch(reqs) => {
                                let pane_id = state.unique_id();
                                let ready_streams = state
                                    .streams
                                    .ready_iter()
                                    .map(|iter| iter.copied().collect::<Vec<_>>())
                                    .unwrap_or_default();

                                fetcher::request_fetch_many(
                                    handles.clone(),
                                    pane_id,
                                    &ready_streams,
                                    *layout_id,
                                    reqs.into_iter().map(|r| (r.req_id, r.fetch, r.stream)),
                                    |handle| {
                                        if let pane::Content::Kline { chart, .. } =
                                            &mut state.content
                                            && let Some(c) = chart
                                        {
                                            c.set_handle(handle);
                                        }
                                    },
                                )
                                .map(Message::from)
                                .chain(self.refresh_streams(main_window.id))
                            }
                            pane::Effect::SwitchTickersInGroup(ticker_info) => {
                                self.switch_tickers_in_group(handles, main_window.id, ticker_info)
                            }
                            pane::Effect::FocusWidget(id) => {
                                return (iced::widget::operation::focus(id), None);
                            }
                            pane::Effect::Ai(message) => {
                                return (Task::none(), Some(Event::Ai(message)));
                            }
                            pane::Effect::Trading(pane_id, message) => {
                                return (Task::done(Message::Trading(pane_id, message)), None);
                            }
                            pane::Effect::TradingLevelDragged(source, id, price, finished) => {
                                return (
                                    Task::done(Message::TradingLevelDragged(
                                        source, id, price, finished,
                                    )),
                                    None,
                                );
                            }
                        };
                        return (task, None);
                    }
                }
            },
            Message::RequestPalette => {
                return (Task::none(), Some(Event::RequestPalette));
            }
            Message::Trading(pane_id, message) => {
                let task = self.update_trading(main_window.id, pane_id, message);
                self.sync_trading_overlays(main_window.id);
                return (task, None);
            }
            Message::TradingLevelDragged(source, id, price, finished) => {
                let task = self.update_trading_level(main_window.id, source, id, price, finished);
                self.sync_trading_overlays(main_window.id);
                return (task, None);
            }
            Message::ChangePaneStatus(pane_id, status) => {
                if let Some(pane_state) = self.get_mut_pane_state_by_uuid(main_window.id, pane_id) {
                    set_fetch_status(&mut pane_state.status, status);
                }
            }
            Message::DistributeFetchedData {
                layout_id,
                pane_id,
                data,
                stream,
            } => {
                return (
                    Task::none(),
                    Some(Event::DistributeFetchedData {
                        layout_id,
                        pane_id,
                        data,
                        stream,
                    }),
                );
            }
            Message::ResolveStreams(pane_id, streams) => {
                return (
                    Task::none(),
                    Some(Event::ResolveStreams { pane_id, streams }),
                );
            }
            Message::Notification(toast) => {
                return (Task::none(), Some(Event::Notification(toast)));
            }
        }

        (Task::none(), None)
    }

    fn new_pane(
        &mut self,
        axis: pane_grid::Axis,
        main_window: &Window,
        pane_state: Option<pane::State>,
    ) -> Task<Message> {
        if self
            .focus
            .filter(|(window, _)| *window == main_window.id)
            .is_some()
        {
            if let Some((_, focused)) = self.focus
                && let Some((pane, _)) =
                    self.panes
                        .split(axis, focused, pane_state.unwrap_or_default())
            {
                return self.focus_pane(main_window.id, pane);
            }
        } else {
            // If there is no focused pane, split the last pane or create a new empty grid
            let pane = self.panes.iter().last().map(|(pane, _)| pane).copied();

            if let Some(pane) = pane {
                let result = self.panes.split(axis, pane, pane_state.unwrap_or_default());

                if let Some((pane, _)) = result {
                    return self.focus_pane(main_window.id, pane);
                }
            } else {
                let (state, pane) = pane_grid::State::new(pane_state.unwrap_or_default());
                self.panes = state;

                return self.focus_pane(main_window.id, pane);
            }
        }

        Task::none()
    }

    fn focus_pane(&mut self, window: window::Id, pane: pane_grid::Pane) -> Task<Message> {
        if self.focus != Some((window, pane)) {
            self.focus = Some((window, pane));
        }

        Task::none()
    }

    fn popout_pane(&mut self, main_window: &Window) -> Task<Message> {
        if let Some((_, id)) = self.focus.take()
            && let Some((pane, _)) = self.panes.close(id)
        {
            let (window, task) = window::open(window::Settings {
                position: main_window
                    .position
                    .map(|point| window::Position::Specific(point + Vector::new(20.0, 20.0)))
                    .unwrap_or_default(),
                exit_on_close_request: false,
                min_size: Some(iced::Size::new(400.0, 300.0)),
                ..window::settings()
            });

            let (state, id) = pane_grid::State::new(pane);
            self.popout.insert(window, (state, WindowSpec::default()));

            return task.then(move |window| {
                Task::done(Message::Pane(window, pane::Message::PaneClicked(id)))
            });
        }

        Task::none()
    }

    fn merge_pane(&mut self, main_window: &Window) -> Task<Message> {
        if let Some((window, pane)) = self.focus.take()
            && let Some(pane_state) = self
                .popout
                .remove(&window)
                .and_then(|(mut panes, _)| panes.panes.remove(&pane))
        {
            let task = self.new_pane(pane_grid::Axis::Horizontal, main_window, Some(pane_state));

            return Task::batch(vec![window::close(window), task]);
        }

        Task::none()
    }

    pub fn get_pane(
        &self,
        main_window: window::Id,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Option<&pane::State> {
        if main_window == window {
            self.panes.get(pane)
        } else {
            self.popout
                .get(&window)
                .and_then(|(panes, _)| panes.get(pane))
        }
    }

    pub fn analysis_sources(&self, main_window: window::Id) -> Vec<crate::ai::ChartSource> {
        let ai_group = self.iter_all_panes(main_window).find_map(|(_, _, state)| {
            matches!(&state.content, pane::Content::AiAnalysis)
                .then_some(state.link_group)
                .flatten()
        });
        self.iter_all_panes(main_window)
            .filter_map(|(_, _, state)| match &state.content {
                pane::Content::Kline {
                    chart: Some(chart),
                    kind,
                    ..
                } if ai_group.is_none() || state.link_group == ai_group => {
                    let ticker_info = chart.ticker_info();
                    let chart_kind =
                        if matches!(kind, data::chart::KlineChartKind::Footprint { .. }) {
                            "Footprint"
                        } else {
                            "Kline"
                        };
                    let (symbol, market) = ticker_info.ticker.display_symbol_and_type();
                    Some(crate::ai::ChartSource {
                        pane_id: state.unique_id(),
                        label: format!(
                            "{symbol} {} | {:?} | {} | {chart_kind}",
                            market_label(market),
                            ticker_info.exchange().venue(),
                            chart.basis()
                        ),
                        symbol: ticker_info.ticker.to_string(),
                        venue: format!("{:?}", ticker_info.exchange()),
                        basis: chart.basis().to_string(),
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn trading_sources(&self, main_window: window::Id) -> Vec<crate::trading::ChartSource> {
        self.iter_all_panes(main_window)
            .filter_map(|(_, _, state)| match &state.content {
                pane::Content::Kline {
                    chart: Some(chart),
                    kind,
                    ..
                } => {
                    let (symbol, market) = chart.ticker_info().ticker.display_symbol_and_type();
                    let (market_latency_ms, best_bid, best_ask) = chart.trading_market_info();
                    let chart_kind =
                        if matches!(kind, data::chart::KlineChartKind::Footprint { .. }) {
                            "Footprint"
                        } else {
                            "Kline"
                        };
                    Some(crate::trading::ChartSource {
                        pane_id: state.unique_id(),
                        link_group: state.link_group,
                        binance_usdm: chart.ticker_info().exchange().venue()
                            == exchange::adapter::Venue::Binance
                            && chart.ticker_info().market_type()
                                == exchange::adapter::MarketKind::LinearPerps,
                        label: format!(
                            "{symbol} {} | {:?} | {} | {chart_kind}",
                            market_label(market),
                            chart.ticker_info().exchange().venue(),
                            chart.basis()
                        ),
                        symbol,
                        min_tick: chart.ticker_info().min_ticksize.as_f64(),
                        min_qty: chart.ticker_info().min_qty.as_f64(),
                        latest_price: chart
                            .alert_observation(state.unique_id())
                            .map(|value| value.price),
                        market_latency_ms,
                        best_bid,
                        best_ask,
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn sync_trading(&mut self, main_window: window::Id, proxy: Option<exchange::proxy::Proxy>) {
        let sources = self.trading_sources(main_window);
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| {
                let link_group = state.link_group;
                if let Some(trader) = state.trader_mut() {
                    trader.set_proxy(proxy.clone());
                    trader.set_chart_sources(
                        sources
                            .iter()
                            .filter(|source| {
                                link_group.is_none() || source.link_group == link_group
                            })
                            .cloned()
                            .collect(),
                    );
                }
            });
        self.sync_trading_overlays(main_window);
    }

    pub fn tick_trading(&mut self, main_window: window::Id, now: Instant) -> Task<Message> {
        Task::batch(
            self.iter_all_panes_mut(main_window)
                .filter_map(|(_, _, state)| {
                    let pane_id = state.unique_id();
                    let task = state.trader_mut()?.tick(now);
                    Some(task.map(move |message| Message::Trading(pane_id, message)))
                }),
        )
    }

    fn update_trading(
        &mut self,
        main_window: window::Id,
        pane_id: uuid::Uuid,
        message: crate::trading::Message,
    ) -> Task<Message> {
        self.get_mut_pane_state_by_uuid(main_window, pane_id)
            .and_then(pane::State::trader_mut)
            .map(|trader| {
                trader
                    .update(message)
                    .map(move |message| Message::Trading(pane_id, message))
            })
            .unwrap_or_else(Task::none)
    }

    fn update_trading_level(
        &mut self,
        main_window: window::Id,
        source: uuid::Uuid,
        id: crate::trading::TradingLineId,
        price: f64,
        finished: bool,
    ) -> Task<Message> {
        Task::batch(
            self.iter_all_panes_mut(main_window)
                .filter_map(|(_, _, state)| {
                    let pane_id = state.unique_id();
                    let trader = state.trader_mut()?;
                    (trader.selected_chart() == Some(source)).then(|| {
                        trader
                            .update(crate::trading::Message::ChartLevelDragged(
                                id, price, finished,
                            ))
                            .map(move |message| Message::Trading(pane_id, message))
                    })
                }),
        )
    }

    fn sync_trading_overlays(&mut self, main_window: window::Id) {
        let overlays = self
            .iter_all_panes(main_window)
            .filter_map(|(_, _, state)| {
                let trader = state.trader()?;
                Some((trader.selected_chart()?, trader.overlay()))
            })
            .collect::<HashMap<_, _>>();
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| {
                let pane_id = state.unique_id();
                if let pane::Content::Kline {
                    chart: Some(chart), ..
                } = &mut state.content
                {
                    chart.set_trading_overlay(overlays.get(&pane_id).cloned().flatten());
                }
            });
    }

    pub fn alert_sources(
        &self,
        main_window: window::Id,
        layout_id: uuid::Uuid,
    ) -> Vec<crate::alerts::Source> {
        let charts = self
            .iter_all_panes(main_window)
            .filter_map(|(_, _, state)| match &state.content {
                pane::Content::Kline {
                    chart: Some(chart),
                    kind,
                    ..
                } => {
                    let ticker = chart.ticker_info().ticker.to_string();
                    let kind = if matches!(kind, data::chart::KlineChartKind::Footprint { .. }) {
                        "Footprint"
                    } else {
                        "Kline"
                    };
                    let basis = chart.basis().to_string();
                    Some((
                        chart.ticker_info(),
                        crate::alerts::Source {
                            key: format!("{ticker}|{basis}|{kind}"),
                            label: format!("{ticker} | {basis} | {kind}"),
                            layout_id,
                            pane_id: state.unique_id(),
                            ai_source_pane: Some(state.unique_id()),
                        },
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut sources = charts
            .iter()
            .map(|(_, source)| source.clone())
            .collect::<Vec<_>>();
        sources.extend(
            self.iter_all_panes(main_window)
                .filter_map(|(_, _, state)| {
                    let pane::Content::Ladder(Some(ladder)) = &state.content else {
                        return None;
                    };
                    let ticker_info = ladder.ticker_info();
                    let ticker = ticker_info.ticker.to_string();
                    let ai_source_pane = charts
                        .iter()
                        .find(|(chart_ticker, _)| *chart_ticker == ticker_info)
                        .map(|(_, source)| source.pane_id);
                    Some(crate::alerts::Source {
                        key: format!("{ticker}|DOM"),
                        label: format!("{ticker} | DOM"),
                        layout_id,
                        pane_id: state.unique_id(),
                        ai_source_pane,
                    })
                }),
        );
        sources
    }

    pub fn alert_observations(&self, main_window: window::Id) -> Vec<crate::alerts::Observation> {
        self.iter_all_panes(main_window)
            .filter_map(|(_, _, state)| match &state.content {
                pane::Content::Kline {
                    chart: Some(chart), ..
                } => chart.alert_observation(state.unique_id()),
                pane::Content::Ladder(Some(ladder)) => ladder.alert_observation(state.unique_id()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn set_ai_annotation(
        &mut self,
        main_window: window::Id,
        source_pane: uuid::Uuid,
        annotation: crate::ai::ChartAnnotation,
    ) {
        if let Some(state) = self.get_mut_pane_state_by_uuid(main_window, source_pane)
            && let pane::Content::Kline {
                chart: Some(chart), ..
            } = &mut state.content
        {
            chart.set_ai_annotation(annotation);
        }
    }

    pub(crate) fn clear_ai_annotations(&mut self, main_window: window::Id) {
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| {
                if let pane::Content::Kline {
                    chart: Some(chart), ..
                } = &mut state.content
                {
                    chart.clear_ai_annotation();
                }
            });
    }

    pub fn analysis_snapshot(
        &self,
        main_window: window::Id,
        source_pane: Option<uuid::Uuid>,
        short_bars: usize,
        long_bars: usize,
        exclude_current_bar: bool,
    ) -> Option<Result<crate::ai::AnalysisBundle, String>> {
        let selected = source_pane.and_then(|source_pane| {
            self.iter_all_panes(main_window).find_map(|(_, _, state)| {
                (state.unique_id() == source_pane)
                    .then(|| match &state.content {
                        pane::Content::Kline {
                            chart: Some(chart), ..
                        } => Some(chart),
                        _ => None,
                    })
                    .flatten()
            })
        });
        if source_pane.is_some() && selected.is_none() {
            return Some(Err(
                "The selected source pane is no longer in the active layout. Refresh sources."
                    .to_string(),
            ));
        }
        let focused = self.focus.and_then(|(window, pane)| {
            self.get_pane(main_window, window, pane)
                .and_then(|state| match &state.content {
                    pane::Content::Kline {
                        chart: Some(chart), ..
                    } => Some(chart),
                    _ => None,
                })
        });

        let primary_chart = selected.or(focused).or_else(|| {
            self.iter_all_panes(main_window)
                .find_map(|(_, _, state)| match &state.content {
                    pane::Content::Kline {
                        chart: Some(chart), ..
                    } => Some(chart),
                    _ => None,
                })
        })?;
        let primary =
            match primary_chart.market_snapshot(short_bars, long_bars, exclude_current_bar) {
                Ok(snapshot) => snapshot,
                Err(error) => return Some(Err(error)),
            };
        let mut higher_timeframe_contexts = self
            .iter_all_panes(main_window)
            .filter_map(|(_, _, state)| match &state.content {
                pane::Content::Kline {
                    chart: Some(chart), ..
                } if !std::ptr::eq(chart, primary_chart) => Some(chart),
                _ => None,
            })
            .filter_map(|chart| chart.market_snapshot(20, long_bars.min(60), false).ok())
            .filter(|snapshot| {
                snapshot.symbol == primary.symbol
                    && snapshot.venue == primary.venue
                    && snapshot.basis != primary.basis
                    && snapshot.timeframe_ms > primary.timeframe_ms
            })
            .collect::<Vec<_>>();
        higher_timeframe_contexts.sort_by_key(|snapshot| snapshot.timeframe_ms);
        higher_timeframe_contexts.truncate(3);

        Some(Ok(crate::ai::AnalysisBundle {
            primary,
            higher_timeframe_contexts,
        }))
    }

    fn get_mut_pane(
        &mut self,
        main_window: window::Id,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Option<&mut pane::State> {
        if main_window == window {
            self.panes.get_mut(pane)
        } else {
            self.popout
                .get_mut(&window)
                .and_then(|(panes, _)| panes.get_mut(pane))
        }
    }

    fn get_mut_pane_state_by_uuid(
        &mut self,
        main_window: window::Id,
        uuid: uuid::Uuid,
    ) -> Option<&mut pane::State> {
        self.iter_all_panes_mut(main_window)
            .find(|(_, _, state)| state.unique_id() == uuid)
            .map(|(_, _, state)| state)
    }

    fn iter_all_panes(
        &self,
        main_window: window::Id,
    ) -> impl Iterator<Item = (window::Id, pane_grid::Pane, &pane::State)> {
        self.panes
            .iter()
            .map(move |(pane, state)| (main_window, *pane, state))
            .chain(self.popout.iter().flat_map(|(window_id, (panes, _))| {
                panes.iter().map(|(pane, state)| (*window_id, *pane, state))
            }))
    }

    fn iter_all_panes_mut(
        &mut self,
        main_window: window::Id,
    ) -> impl Iterator<Item = (window::Id, pane_grid::Pane, &mut pane::State)> {
        self.panes
            .iter_mut()
            .map(move |(pane, state)| (main_window, *pane, state))
            .chain(self.popout.iter_mut().flat_map(|(window_id, (panes, _))| {
                panes
                    .iter_mut()
                    .map(|(pane, state)| (*window_id, *pane, state))
            }))
    }

    pub fn view<'a>(
        &'a self,
        main_window: &'a Window,
        tickers_table: &'a TickersTable,
        timezone: UserTimezone,
        ai_agent: &'a crate::ai::Agent,
    ) -> Element<'a, Message> {
        let pane_grid: Element<_> = PaneGrid::new(&self.panes, |id, pane, maximized| {
            let is_focused = self.focus == Some((main_window.id, id));
            pane.view(
                id,
                self.panes.len(),
                is_focused,
                maximized,
                main_window.id,
                main_window,
                timezone,
                tickers_table,
                ai_agent,
            )
        })
        .min_size(240)
        .on_click(pane::Message::PaneClicked)
        .on_drag(pane::Message::PaneDragged)
        .on_resize(8, pane::Message::PaneResized)
        .spacing(6)
        .style(style::pane_grid)
        .into();

        pane_grid.map(move |message| Message::Pane(main_window.id, message))
    }

    pub fn view_window<'a>(
        &'a self,
        window: window::Id,
        main_window: &'a Window,
        tickers_table: &'a TickersTable,
        timezone: UserTimezone,
        ai_agent: &'a crate::ai::Agent,
    ) -> Element<'a, Message> {
        if let Some((state, _)) = self.popout.get(&window) {
            let content = container(
                PaneGrid::new(state, |id, pane, _maximized| {
                    let is_focused = self.focus == Some((window, id));
                    pane.view(
                        id,
                        state.len(),
                        is_focused,
                        false,
                        window,
                        main_window,
                        timezone,
                        tickers_table,
                        ai_agent,
                    )
                })
                .on_click(pane::Message::PaneClicked),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(8);

            Element::new(content).map(move |message| Message::Pane(window, message))
        } else {
            Element::new(center("No pane found for window"))
                .map(move |message| Message::Pane(window, message))
        }
    }

    pub fn go_back(&mut self, main_window: window::Id) -> bool {
        let Some((window, pane)) = self.focus else {
            return false;
        };

        let Some(state) = self.get_mut_pane(main_window, window, pane) else {
            return false;
        };

        if state.modal.is_some() {
            state.modal = None;
            return true;
        }
        false
    }

    fn handle_error(
        &mut self,
        pane_id: Option<uuid::Uuid>,
        err: &DashboardError,
        main_window: window::Id,
    ) -> Task<Message> {
        match pane_id {
            Some(id) => {
                if let Some(state) = self.get_mut_pane_state_by_uuid(main_window, id) {
                    state.status = pane::Status::Ready;
                    state.notifications.push(Toast::error(err.to_string()));
                }
                Task::none()
            }
            _ => Task::done(Message::Notification(Toast::error(err.to_string()))),
        }
    }

    /// Borrow visual config, studies, and clusters from an existing pane of the same
    /// content kind, so newly created panes inherit the full set of settings
    /// that the manual "Sync all" button would apply.
    fn borrow_synced_settings(
        &self,
        main_window: window::Id,
        content_kind: ContentKind,
    ) -> Option<(
        VisualConfig,
        Option<data::chart::Study>,
        Option<data::chart::kline::ClusterKind>,
    )> {
        self.iter_all_panes(main_window).find_map(|(_, _, state)| {
            if state.content.kind() == content_kind {
                let visual_config = state.settings.visual_config.clone()?;
                let studies = state.content.studies();
                let clusters = state.content.clusters();
                Some((visual_config, studies, clusters))
            } else {
                None
            }
        })
    }

    fn init_pane(
        &mut self,
        handles: &AdapterHandles,
        main_window: window::Id,
        window: window::Id,
        selected_pane: pane_grid::Pane,
        ticker_info: TickerInfo,
        content_kind: ContentKind,
    ) -> Task<Message> {
        let synced = self.borrow_synced_settings(main_window, content_kind);

        let Some(state) = self.get_mut_pane(main_window, window, selected_pane) else {
            return Task::none();
        };

        if state.settings.visual_config.is_none()
            && let Some((cfg, _, _)) = &synced
        {
            state.settings.visual_config = Some(cfg.clone());
        }

        let pane_id = state.unique_id();
        let streams = state.set_content_and_streams(vec![ticker_info], content_kind);

        // Apply synced configs now that content is initialized
        if let Some((_, studies, clusters)) = &synced {
            state.apply_synced_settings(studies, clusters);
        }

        self.streams.extend(streams.iter());

        for stream in &streams {
            if let StreamKind::Kline { .. } = stream {
                return fetcher::kline_fetch_task(
                    handles.clone(),
                    self.layout_id,
                    pane_id,
                    *stream,
                    None,
                    None,
                )
                .map(Message::from);
            }
        }

        Task::none()
    }

    pub fn init_focused_pane(
        &mut self,
        handles: &AdapterHandles,
        main_window: window::Id,
        ticker_info: TickerInfo,
        content_kind: ContentKind,
    ) -> Task<Message> {
        if self.focus.is_none()
            && self.panes.len() == 1
            && let Some((pane_id, _)) = self.panes.iter().next()
        {
            self.focus = Some((main_window, *pane_id));
        }

        // Inherit settings from an existing pane of the same type
        // (mirrors what the manual "Sync all" button does)
        let synced = self.borrow_synced_settings(main_window, content_kind);

        let Some((window, selected_pane)) = self.focus else {
            return Task::done(Message::Notification(Toast::warn(
                "No focused pane found".to_string(),
            )));
        };

        let Some(state) = self.get_mut_pane(main_window, window, selected_pane) else {
            return Task::done(Message::Notification(Toast::warn(
                "No focused pane found".to_string(),
            )));
        };

        if state.settings.visual_config.is_none()
            && let Some((cfg, _, _)) = &synced
        {
            state.settings.visual_config = Some(cfg.clone());
        }

        let previous_ticker = state.stream_pair();
        if previous_ticker.is_some() && previous_ticker != Some(ticker_info) {
            state.link_group = None;
        }

        let streams = state.set_content_and_streams(vec![ticker_info], content_kind);

        if let Some((_, studies, clusters)) = &synced {
            state.apply_synced_settings(studies, clusters);
        }

        let pane_id = state.unique_id();
        self.streams.extend(streams.iter());

        for stream in &streams {
            if let StreamKind::Kline { .. } = stream {
                return fetcher::kline_fetch_task(
                    handles.clone(),
                    self.layout_id,
                    pane_id,
                    *stream,
                    None,
                    None,
                )
                .map(Message::from);
            }
        }

        Task::none()
    }

    pub fn switch_tickers_in_group(
        &mut self,
        handles: &AdapterHandles,
        main_window: window::Id,
        ticker_info: TickerInfo,
    ) -> Task<Message> {
        if self.focus.is_none()
            && self.panes.len() == 1
            && let Some((pane_id, _)) = self.panes.iter().next()
        {
            self.focus = Some((main_window, *pane_id));
        }

        let link_group = self.focus.and_then(|(window, pane)| {
            self.get_pane(main_window, window, pane)
                .and_then(|state| state.link_group)
        });

        if let Some(group) = link_group {
            let pane_infos: Vec<(window::Id, pane_grid::Pane, ContentKind)> = self
                .iter_all_panes_mut(main_window)
                .filter_map(|(window, pane, state)| {
                    if state.link_group == Some(group) {
                        Some((window, pane, state.content.kind()))
                    } else {
                        None
                    }
                })
                .collect();

            let tasks: Vec<Task<Message>> = pane_infos
                .iter()
                .map(|(window, pane, content_kind)| {
                    self.init_pane(
                        handles,
                        main_window,
                        *window,
                        *pane,
                        ticker_info,
                        *content_kind,
                    )
                })
                .collect();

            Task::batch(tasks)
        } else if let Some((window, pane)) = self.focus {
            if let Some(state) = self.get_mut_pane(main_window, window, pane) {
                let content_kind = state.content.kind();
                self.init_focused_pane(handles, main_window, ticker_info, content_kind)
            } else {
                Task::done(Message::Notification(Toast::warn(
                    "Couldn't get focused pane's content".to_string(),
                )))
            }
        } else {
            Task::done(Message::Notification(Toast::warn(
                "No link group or focused pane found".to_string(),
            )))
        }
    }

    pub fn toggle_trade_fetch(&mut self, is_enabled: bool, main_window: &Window) {
        fetcher::toggle_trade_fetch(is_enabled);

        self.iter_all_panes_mut(main_window.id)
            .for_each(|(_, _, state)| {
                if let pane::Content::Kline { chart, kind, .. } = &mut state.content
                    && matches!(kind, data::chart::KlineChartKind::Footprint { .. })
                    && let Some(c) = chart
                {
                    c.reset_request_handler();

                    if !is_enabled {
                        state.status = pane::Status::Ready;
                    }
                }
            });
    }

    pub fn distribute_fetched_data(
        &mut self,
        main_window: window::Id,
        pane_id: uuid::Uuid,
        data: FetchedData,
        stream_type: StreamKind,
    ) -> Task<Message> {
        match data {
            FetchedData::Trades {
                batch,
                until_time,
                req_id,
                is_batches_done,
            } => {
                let last_trade_time = batch.last().map_or(UnixMs::ZERO, |trade| trade.time);
                let reached_end = last_trade_time >= until_time;
                let is_batches_done = is_batches_done || reached_end;

                if !reached_end {
                    if let Err(reason) = self.insert_fetched_trades(
                        main_window,
                        pane_id,
                        &batch,
                        is_batches_done,
                        req_id,
                    ) {
                        return self.handle_error(Some(pane_id), &reason, main_window);
                    }
                } else {
                    let filtered_batch = batch
                        .iter()
                        .filter(|trade| trade.time <= until_time)
                        .copied()
                        .collect::<Vec<_>>();

                    if let Err(reason) = self.insert_fetched_trades(
                        main_window,
                        pane_id,
                        &filtered_batch,
                        true,
                        req_id,
                    ) {
                        return self.handle_error(Some(pane_id), &reason, main_window);
                    }
                }
            }
            FetchedData::Klines { data, req_id } => {
                if let StreamKind::Kline {
                    timeframe,
                    ticker_info,
                } = stream_type
                {
                    if let Some(pane_state) = self.get_mut_pane_state_by_uuid(main_window, pane_id)
                    {
                        set_fetch_status(&mut pane_state.status, pane::Status::Ready);
                        pane_state.insert_hist_klines(req_id, timeframe, ticker_info, &data);
                    }

                    self.iter_all_panes_mut(main_window)
                        .filter(|(_, _, state)| {
                            state.unique_id() != pane_id
                                && state.streams.matches_stream(&stream_type)
                        })
                        .for_each(|(_, _, state)| {
                            state.insert_shared_klines(timeframe, &data);
                        });
                }
            }
            FetchedData::OI { data, req_id } => {
                if let Some(pane_state) = self.get_mut_pane_state_by_uuid(main_window, pane_id) {
                    set_fetch_status(&mut pane_state.status, pane::Status::Ready);

                    if let StreamKind::Kline { .. } = stream_type {
                        pane_state.insert_hist_oi(req_id, &data);
                    }
                }
            }
        }

        Task::none()
    }

    fn insert_fetched_trades(
        &mut self,
        main_window: window::Id,
        pane_id: uuid::Uuid,
        trades: &[Trade],
        is_batches_done: bool,
        req_id: uuid::Uuid,
    ) -> Result<(), DashboardError> {
        let pane_state = self
            .get_mut_pane_state_by_uuid(main_window, pane_id)
            .ok_or_else(|| {
                DashboardError::Unknown(
                    "No matching pane state found for fetched trades".to_string(),
                )
            })?;

        match &mut pane_state.status {
            pane::Status::Loading(InfoKind::FetchingTrades(count)) => {
                *count += trades.len();
            }
            _ => {
                pane_state.status = pane::Status::Loading(InfoKind::FetchingTrades(trades.len()));
            }
        }

        match &mut pane_state.content {
            pane::Content::Kline { chart, .. } => {
                if let Some(c) = chart {
                    c.insert_raw_trades(trades.to_owned(), is_batches_done, req_id);

                    if is_batches_done {
                        pane_state.status = pane::Status::Ready;
                    }
                    Ok(())
                } else {
                    Err(DashboardError::Unknown(
                        "fetched trades but no chart found".to_string(),
                    ))
                }
            }
            _ => Err(DashboardError::Unknown(
                "No matching chart found for fetched trades".to_string(),
            )),
        }
    }

    pub fn update_latest_klines(
        &mut self,
        stream: &StreamKind,
        kline: &Kline,
        main_window: window::Id,
    ) -> Task<Message> {
        let mut found_match = false;

        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, pane_state)| {
                if pane_state.matches_stream(stream) {
                    match &mut pane_state.content {
                        pane::Content::Kline { chart: Some(c), .. } => {
                            c.update_latest_kline(kline);
                        }
                        pane::Content::Comparison(Some(c)) => {
                            c.update_latest_kline(&stream.ticker_info(), kline);
                        }
                        _ => {}
                    }
                    found_match = true;
                }
            });

        if found_match {
            Task::none()
        } else {
            log::debug!("{stream:?} stream had no matching panes - dropping");
            self.refresh_streams(main_window)
        }
    }

    pub fn ingest_depth(
        &mut self,
        stream: &StreamKind,
        update_t: UnixMs,
        depth: &Depth,
        main_window: window::Id,
    ) -> Task<Message> {
        let mut found_match = false;

        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, pane_state)| {
                if pane_state.matches_stream(stream) {
                    match &mut pane_state.content {
                        pane::Content::Heatmap { chart, .. } => {
                            if let Some(c) = chart {
                                c.insert_depth(depth, update_t);
                            }
                        }
                        pane::Content::ShaderHeatmap { chart, .. } => {
                            if let Some(c) = chart {
                                c.insert_depth(depth, update_t);
                            }
                        }
                        pane::Content::Ladder(panel) => {
                            if let Some(panel) = panel {
                                panel.insert_depth(depth, update_t);
                            }
                        }
                        pane::Content::Kline { chart, .. } => {
                            if let Some(chart) = chart {
                                chart.insert_depth(depth, update_t);
                            }
                        }
                        _ => {
                            log::error!("No chart found for the stream: {stream:?}");
                        }
                    }
                    found_match = true;
                }
            });

        if found_match {
            Task::none()
        } else {
            self.refresh_streams(main_window)
        }
    }

    pub fn ingest_liquidations(
        &mut self,
        stream: &StreamKind,
        liquidations: &[Liquidation],
        main_window: window::Id,
    ) -> Task<Message> {
        let mut found_match = false;
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, pane_state)| {
                if pane_state.matches_stream(stream) {
                    if let pane::Content::Kline {
                        chart: Some(chart), ..
                    } = &mut pane_state.content
                    {
                        chart.insert_liquidations(liquidations);
                    }
                    found_match = true;
                }
            });
        if found_match {
            Task::none()
        } else {
            self.refresh_streams(main_window)
        }
    }

    pub fn ingest_trades(
        &mut self,
        stream: &StreamKind,
        buffer: &[Trade],
        update_t: UnixMs,
        main_window: window::Id,
    ) -> Task<Message> {
        let mut found_match = false;

        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, pane_state)| {
                if pane_state.matches_stream(stream) {
                    match &mut pane_state.content {
                        pane::Content::Heatmap { chart, .. } => {
                            if let Some(c) = chart {
                                c.insert_trades(buffer, update_t);
                            }
                        }
                        pane::Content::ShaderHeatmap { chart, .. } => {
                            if let Some(c) = chart {
                                c.insert_trades(buffer, update_t);
                            }
                        }
                        pane::Content::Kline { chart, .. } => {
                            if let Some(c) = chart {
                                c.insert_trades(buffer, update_t);
                            }
                        }
                        pane::Content::TimeAndSales(panel) => {
                            if let Some(p) = panel {
                                p.insert_buffer(buffer);
                            }
                        }
                        pane::Content::Ladder(panel) => {
                            if let Some(p) = panel {
                                p.insert_trades(buffer);
                            }
                        }
                        _ => {
                            log::error!("No chart found for the stream: {stream:?}");
                        }
                    }
                    found_match = true;
                }
            });

        if found_match {
            Task::none()
        } else {
            self.refresh_streams(main_window)
        }
    }

    pub fn invalidate_all_panes(&mut self, main_window: window::Id) {
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| {
                let _ = state.invalidate(Instant::now());
            });
    }

    pub fn park_for_inactive_layout(&mut self, main_window: window::Id) {
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| state.park_for_inactive_layout());
    }

    pub fn tick(
        &mut self,
        handles: &AdapterHandles,
        now: Instant,
        _main_window: window::Id,
    ) -> Task<Message> {
        let mut tasks = vec![];

        let mut tick_state = |state: &mut pane::State| match state.tick(now) {
            Some(pane::Action::Chart(action)) => match action {
                chart::Action::ErrorOccurred(err) => {
                    state.status = pane::Status::Ready;
                    state.notifications.push(Toast::error(err.to_string()));
                }
                chart::Action::RequestFetch(reqs) => {
                    let pane_id = state.unique_id();
                    let ready_streams = state
                        .streams
                        .ready_iter()
                        .map(|iter| iter.copied().collect::<Vec<_>>())
                        .unwrap_or_default();

                    let fetch_tasks = fetcher::request_fetch_many(
                        handles.clone(),
                        pane_id,
                        &ready_streams,
                        self.layout_id,
                        reqs.into_iter().map(|r| (r.req_id, r.fetch, r.stream)),
                        |handle| {
                            if let pane::Content::Kline { chart, .. } = &mut state.content
                                && let Some(c) = chart
                            {
                                c.set_handle(handle);
                            }
                        },
                    )
                    .map(Message::from);

                    tasks.push(fetch_tasks);
                }
                chart::Action::RequestPalette => {
                    tasks.push(Task::done(Message::RequestPalette));
                }
            },
            Some(pane::Action::Panel(_action)) => {}
            Some(pane::Action::ResolveStreams(streams)) => {
                tasks.push(Task::done(Message::ResolveStreams(
                    state.unique_id(),
                    streams,
                )));
            }
            Some(pane::Action::ResolveContent) => match state.stream_pair_kind() {
                Some(StreamPairKind::MultiSource(tickers)) => {
                    state.set_content_and_streams(tickers, state.content.kind());
                }
                Some(StreamPairKind::SingleSource(ticker)) => {
                    state.set_content_and_streams(vec![ticker], state.content.kind());
                }
                None => {}
            },
            None => {}
        };

        // tick only the maximized pane if there is any, otherwise tick all panes
        let maximized_pane = self.panes.maximized();
        for (pane_id, state) in self.panes.iter_mut() {
            if maximized_pane.is_some_and(|maximized| *pane_id != maximized) {
                continue;
            }

            tick_state(state);
        }

        for (popout_state, _) in self.popout.values_mut() {
            for (_, state) in popout_state.iter_mut() {
                tick_state(state);
            }
        }

        Task::batch(tasks)
    }

    pub fn resolve_streams(
        &mut self,
        main_window: window::Id,
        pane_id: uuid::Uuid,
        mut streams: Vec<StreamKind>,
    ) -> Task<Message> {
        if let Some(state) = self.get_mut_pane_state_by_uuid(main_window, pane_id) {
            if matches!(state.content, pane::Content::Kline { .. })
                && let Some(ticker_info) = streams.first().map(StreamKind::ticker_info)
                && ticker_info.is_perps()
                && ticker_info.exchange().venue() == exchange::adapter::Venue::Binance
            {
                let stream = StreamKind::Liquidations { ticker_info };
                if !streams.contains(&stream) {
                    streams.push(stream);
                }
            }
            state.streams = ResolvedStream::Ready(streams.clone());
        }
        self.refresh_streams(main_window)
    }

    pub fn block_streams(&mut self, main_window: window::Id, pane_id: uuid::Uuid, reason: String) {
        if let Some(state) = self.get_mut_pane_state_by_uuid(main_window, pane_id) {
            match &mut state.streams {
                ResolvedStream::Waiting { streams, .. } => {
                    state.streams = ResolvedStream::Blocked {
                        streams: streams.clone(),
                        reason,
                        last_attempt: None,
                    };
                }
                ResolvedStream::Blocked {
                    reason: old_reason, ..
                } => {
                    *old_reason = reason;
                }
                _ => {}
            }
        }
    }

    pub fn market_subscriptions(
        streams: &UniqueStreams,
        handles: &AdapterHandles,
    ) -> Subscription<exchange::Event> {
        let unique_streams = streams
            .combined_used()
            .flat_map(|(exchange, specs)| {
                let mut subs = vec![];

                if !specs.depth.is_empty() {
                    let depth_subs = specs
                        .depth
                        .iter()
                        .map(|(ticker, aggr, push_freq)| {
                            let tick_mltp = match aggr {
                                StreamTicksize::Client => None,
                                StreamTicksize::ServerSide(tick_mltp) => Some(*tick_mltp),
                            };

                            let config = StreamConfig::new(
                                *ticker,
                                ticker.exchange(),
                                tick_mltp,
                                *push_freq,
                            );

                            let data = (handles.clone(), config);
                            Subscription::run_with(data, |data| data.0.depth_stream(&data.1))
                        })
                        .collect::<Vec<_>>();

                    if !depth_subs.is_empty() {
                        subs.push(Subscription::batch(depth_subs));
                    }
                }

                if !specs.trade.is_empty() {
                    let trade_subs = specs
                        .trade
                        .chunks(MAX_TRADE_TICKERS_PER_STREAM)
                        .map(|tickers| {
                            let config = StreamConfig::new(
                                tickers.to_vec(),
                                exchange,
                                None,
                                PushFrequency::ServerDefault,
                            );

                            let data = (handles.clone(), config);
                            Subscription::run_with(data, |data| data.0.trade_stream(&data.1))
                        })
                        .collect::<Vec<_>>();

                    if !trade_subs.is_empty() {
                        subs.push(Subscription::batch(trade_subs));
                    }
                }

                if !specs.kline.is_empty() {
                    let kline_subs = specs
                        .kline
                        .chunks(MAX_KLINE_STREAMS_PER_STREAM)
                        .map(|streams| {
                            let config = StreamConfig::new(
                                streams.to_vec(),
                                exchange,
                                None,
                                PushFrequency::ServerDefault,
                            );

                            let data = (handles.clone(), config);
                            Subscription::run_with(data, |data| data.0.kline_stream(&data.1))
                        })
                        .collect::<Vec<_>>();

                    if !kline_subs.is_empty() {
                        subs.push(Subscription::batch(kline_subs));
                    }
                }

                if !specs.liquidation.is_empty() {
                    let config = StreamConfig::new(
                        specs.liquidation.clone(),
                        exchange,
                        None,
                        PushFrequency::ServerDefault,
                    );
                    let data = (handles.clone(), config);
                    subs.push(Subscription::run_with(data, |data| {
                        data.0.liquidation_stream(&data.1)
                    }));
                }

                subs
            })
            .collect::<Vec<Subscription<exchange::Event>>>();

        Subscription::batch(unique_streams)
    }

    pub fn theme_updated(&mut self, main_window: window::Id, theme: &iced_core::Theme) {
        self.iter_all_panes_mut(main_window)
            .for_each(|(_, _, state)| {
                state.content.update_theme(theme);
            });
    }

    fn refresh_streams(&mut self, main_window: window::Id) -> Task<Message> {
        let all_pane_streams = self
            .iter_all_panes(main_window)
            .flat_map(|(_, _, pane_state)| pane_state.streams.ready_iter().into_iter().flatten());
        self.streams = UniqueStreams::from(all_pane_streams);

        Task::none()
    }
}

impl From<fetcher::FetchUpdate> for Message {
    fn from(update: fetcher::FetchUpdate) -> Self {
        match update {
            fetcher::FetchUpdate::Status { pane_id, status } => match status {
                fetcher::FetchTaskStatus::Loading(info) => {
                    Message::ChangePaneStatus(pane_id, pane::Status::Loading(info))
                }
                fetcher::FetchTaskStatus::Completed => {
                    Message::ChangePaneStatus(pane_id, pane::Status::Ready)
                }
            },
            fetcher::FetchUpdate::Data {
                layout_id,
                pane_id,
                stream,
                data,
            } => Message::DistributeFetchedData {
                layout_id,
                pane_id,
                stream,
                data,
            },
            fetcher::FetchUpdate::Error {
                pane_id,
                req_id,
                error,
            } => Message::FetchFailed {
                pane_id,
                req_id,
                error,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_fetch_progress_is_not_hidden_by_other_fetches() {
        let mut status = pane::Status::Loading(InfoKind::FetchingTrades(42));

        set_fetch_status(&mut status, pane::Status::Loading(InfoKind::FetchingOI));
        assert_eq!(status, pane::Status::Loading(InfoKind::FetchingTrades(42)));

        set_fetch_status(
            &mut status,
            pane::Status::Loading(InfoKind::FetchingTrades(84)),
        );
        assert_eq!(status, pane::Status::Loading(InfoKind::FetchingTrades(84)));
    }
}
