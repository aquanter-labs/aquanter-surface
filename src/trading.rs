use crate::i18n::tr;
use data::config::trading::{TradingConfig, TradingCredentials, TradingExchange};
use hmac::{Hmac, Mac};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, rule, slider, text, text_input, tooltip,
};
use iced::{Alignment, Element, Length, Task, padding};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

mod event;
pub use event::{EventContract, EventDuration};

const MAINNET_URL: &str = "https://fapi.binance.com";
const TESTNET_URL: &str = "https://demo-fapi.binance.com";
// ponytail: simulation uses Binance's first maintenance tier; fetch signed leverage brackets for tiered cross-margin parity.
const SIM_MAINTENANCE_MARGIN_RATE: f64 = 0.005;
const DEFAULT_LIQUIDATION_FEE_RATE: f64 = 0.0125;

fn default_liquidation_fee_rate() -> f64 {
    DEFAULT_LIQUIDATION_FEE_RATE
}

fn labeled_input<'a>(
    label: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: fn(String) -> Message,
) -> Element<'a, Message> {
    column![
        text(tr(label)).size(crate::style::text_size::SMALL),
        text_input(tr(placeholder), value).on_input(on_input),
    ]
    .spacing(2)
    .into()
}

fn icon_button<'a>(
    icon: crate::style::LucideIcon,
    label: &'static str,
    message: Message,
) -> Element<'a, Message> {
    tooltip(
        button(
            crate::style::lucide_icon_text(icon, 13)
                .width(18)
                .align_x(Alignment::Center),
        )
        .padding(4)
        .style(|theme, status| crate::style::button::transparent(theme, status, false))
        .on_press(message),
        Some(tr(label)),
        tooltip::Position::Bottom,
    )
    .into()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Side {
    #[default]
    Buy,
    Sell,
}

impl Side {
    fn api(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

fn trade_actions(position_side: PositionSide) -> [(Side, bool, &'static str); 2] {
    match position_side {
        PositionSide::Long | PositionSide::Both => [
            (Side::Buy, false, tr("Buy / Open")),
            (Side::Sell, true, tr("Sell / Close")),
        ],
        PositionSide::Short => [
            (Side::Sell, false, tr("Sell / Open")),
            (Side::Buy, true, tr("Buy / Close")),
        ],
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Buy => tr("Buy / Long"),
            Self::Sell => tr("Sell / Short"),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum OrderType {
    #[default]
    Market,
    Limit,
}

impl OrderType {
    pub const ALL: [Self; 2] = [Self::Market, Self::Limit];
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Market => tr("Market"),
            Self::Limit => tr("Limit"),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum MarginMode {
    #[default]
    Cross,
    Isolated,
}

impl MarginMode {
    pub const ALL: [Self; 2] = [Self::Cross, Self::Isolated];

    fn api(self) -> &'static str {
        match self {
            Self::Cross => "CROSSED",
            Self::Isolated => "ISOLATED",
        }
    }
}

impl std::fmt::Display for MarginMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cross => tr("Cross"),
            Self::Isolated => tr("Isolated"),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum PositionSide {
    #[default]
    Both,
    Long,
    Short,
}

impl PositionSide {
    const TRADE: [Self; 2] = [Self::Long, Self::Short];

    fn api(self) -> &'static str {
        match self {
            Self::Both => "BOTH",
            Self::Long => "LONG",
            Self::Short => "SHORT",
        }
    }
}

impl std::fmt::Display for PositionSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Both => tr("Both"),
            Self::Long => tr("Long"),
            Self::Short => tr("Short"),
        })
    }
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub position_side: PositionSide,
    pub order_type: OrderType,
    pub quantity: f64,
    pub price: Option<f64>,
    pub reduce_only: bool,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
}

impl OrderRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.symbol.trim().is_empty() {
            return Err(tr("Symbol is required.").to_string());
        }
        if !self.quantity.is_finite() || self.quantity <= 0.0 {
            return Err(tr("Quantity must be greater than zero.").to_string());
        }
        if self.order_type == OrderType::Limit
            && !self
                .price
                .is_some_and(|price| price.is_finite() && price > 0.0)
        {
            return Err(tr("A valid price is required for a limit order.").to_string());
        }
        for (name, value) in [
            (tr("Stop loss"), self.stop_loss),
            (tr("Take profit"), self.take_profit),
        ] {
            if value.is_some_and(|price| !price.is_finite() || price <= 0.0) {
                return Err(format!("{name} must be a positive price."));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Balance {
    pub asset: String,
    pub wallet: f64,
    pub available: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Position {
    pub symbol: String,
    pub side: PositionSide,
    pub quantity: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub break_even_price: f64,
    pub economic_break_even_price: f64,
    pub liquidation_price: f64,
    pub leverage: u16,
    pub margin_mode: MarginMode,
    pub unrealized_pnl: f64,
    pub funding: f64,
    pub opened_at_ms: Option<u64>,
}

impl Position {
    pub fn is_long(&self) -> bool {
        self.quantity > 0.0
    }

    pub fn estimated_pnl_at(&self, price: f64, exit_fee_rate: f64) -> f64 {
        self.quantity * (price - self.economic_break_even_price)
            - self.quantity.abs() * price * exit_fee_rate.max(0.0)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenOrder {
    pub symbol: String,
    pub order_id: u64,
    pub side: Side,
    pub order_type: String,
    pub price: f64,
    pub stop_price: f64,
    pub quantity: f64,
    pub reduce_only: bool,
    pub is_algo: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeHistory {
    pub trade_id: u64,
    pub order_id: u64,
    pub symbol: String,
    pub side: Side,
    pub quantity: f64,
    pub price: f64,
    pub realized_pnl: f64,
    pub commission: f64,
    pub commission_asset: String,
    pub time_ms: u64,
}

impl TradeHistory {
    fn net_pnl_usdt(&self) -> f64 {
        self.realized_pnl
            - if self.commission_asset == "USDT" {
                self.commission
            } else {
                0.0
            }
    }
}

fn account_analytics(history: &[TradeHistory]) -> AccountAnalytics {
    let mut analytics = AccountAnalytics {
        net_pnl: history.iter().map(TradeHistory::net_pnl_usdt).sum(),
        ..AccountAnalytics::default()
    };
    for pnl in history
        .iter()
        .filter(|trade| trade.realized_pnl.abs() > f64::EPSILON)
        .map(TradeHistory::net_pnl_usdt)
    {
        analytics.closed_fills += 1;
        if pnl > 0.0 {
            analytics.wins += 1;
            analytics.gross_profit += pnl;
            analytics.best_trade = analytics.best_trade.max(pnl);
        } else if pnl < 0.0 {
            analytics.losses += 1;
            analytics.gross_loss += -pnl;
            analytics.worst_trade = analytics.worst_trade.min(pnl);
        }
    }
    analytics.average_win = if analytics.wins > 0 {
        analytics.gross_profit / analytics.wins as f64
    } else {
        0.0
    };
    analytics.average_loss = if analytics.losses > 0 {
        -(analytics.gross_loss / analytics.losses as f64)
    } else {
        0.0
    };
    analytics
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct CommissionRate {
    pub maker: f64,
    pub taker: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AccountSnapshot {
    pub balances: Vec<Balance>,
    pub positions: Vec<Position>,
    pub open_orders: Vec<OpenOrder>,
    #[serde(default)]
    pub trade_history: Vec<TradeHistory>,
    /// Simulated event contracts, open and settled.
    #[serde(default)]
    pub event_contracts: Vec<EventContract>,
    pub commission: CommissionRate,
    #[serde(default = "default_liquidation_fee_rate")]
    pub liquidation_fee_rate: f64,
    pub hedge_mode: bool,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PlacedOrder {
    pub symbol: String,
    pub order_id: u64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSource {
    pub pane_id: uuid::Uuid,
    pub link_group: Option<data::layout::pane::LinkGroup>,
    pub binance_usdm: bool,
    pub label: String,
    pub symbol: String,
    pub min_tick: f64,
    pub min_qty: f64,
    pub latest_price: Option<f64>,
    pub market_latency_ms: Option<u64>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
}

impl std::fmt::Display for ChartSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Debug, Clone)]
pub struct PositionOverlay {
    pub symbol: String,
    pub is_long: bool,
    pub entry_price: Option<f64>,
    pub break_even_price: Option<f64>,
    /// Draw `break_even_price` instead of `entry_price`.
    pub price_with_fees: bool,
    /// Fills drawn as B/S badges on their candle rather than arrows at the price.
    pub fill_badges: bool,
    /// Settled event contracts drawn as W/L badges on their expiry candle.
    pub event_badges: bool,
    /// TP / SL and limit lines accept drags.
    pub draggable: bool,
    /// A chart edit waiting for confirmation, shown as a popup on the chart.
    pub confirm: Option<String>,
    pub liquidation_price: Option<f64>,
    pub liquidation_estimated: bool,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub stop_loss_pnl: Option<f64>,
    pub take_profit_pnl: Option<f64>,
    pub unrealized_pnl: f64,
    pub orders: Vec<TradingOrderOverlay>,
    pub trade_history: Vec<TradeMarker>,
    pub event_contracts: Vec<EventContract>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingLineId {
    StopLoss,
    TakeProfit,
    LimitOrder { order_id: u64 },
}

#[derive(Debug, Clone)]
pub struct TradingOrderOverlay {
    pub id: TradingLineId,
    pub label: String,
    pub price: f64,
    pub side: Side,
}

#[derive(Debug, Clone)]
pub struct TradeMarker {
    pub time_ms: u64,
    pub price: f64,
    pub side: Side,
}

#[derive(Debug, Clone)]
struct ProtectionEditor {
    symbol: String,
    stop_loss: String,
    take_profit: String,
}

#[derive(Debug, Default, PartialEq)]
struct AccountAnalytics {
    closed_fills: usize,
    wins: usize,
    losses: usize,
    net_pnl: f64,
    gross_profit: f64,
    gross_loss: f64,
    average_win: f64,
    average_loss: f64,
    best_trade: f64,
    worst_trade: f64,
}

impl AccountAnalytics {
    fn win_rate(&self) -> f64 {
        if self.closed_fills == 0 {
            0.0
        } else {
            self.wins as f64 / self.closed_fills as f64 * 100.0
        }
    }

    fn profit_factor(&self) -> f64 {
        if self.gross_loss > f64::EPSILON {
            self.gross_profit / self.gross_loss
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone)]
enum PendingAction {
    Place {
        request: OrderRequest,
        leverage: u16,
        margin_mode: MarginMode,
    },
    Cancel {
        symbol: String,
        order_id: u64,
        is_algo: bool,
    },
    CancelAll {
        symbol: String,
    },
    Close(Position),
    Protect {
        position: Position,
        stop_loss: Option<f64>,
        take_profit: Option<f64>,
        existing: Vec<OpenOrder>,
    },
    ModifyLimit {
        order: OpenOrder,
        price: f64,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    AccountLabelChanged(String),
    ExchangeChanged(TradingExchange),
    SimulationBalanceChanged(String),
    SimulationMakerFeeChanged(String),
    SimulationTakerFeeChanged(String),
    ResetSimulation,
    ApiKeyChanged(String),
    SecretKeyChanged(String),
    ConfirmOrdersChanged(bool),
    ChartPriceWithFeesChanged(bool),
    ChartShowPositionChanged(bool),
    ChartShowFillsChanged(bool),
    ChartFillBadgesChanged(bool),
    EventChartBadgesChanged(bool),
    ChartOpeningFillsOnlyChanged(bool),
    ChartDragOrdersChanged(bool),
    ChartDragConfirmChanged(bool),
    SaveCredentials,
    Connect,
    Disconnect,
    Refresh,
    Poll,
    Connected(Result<(BinanceClient, AccountSnapshot), String>),
    SnapshotLoaded(Result<AccountSnapshot, String>),
    LiquidationFeeLoaded(Result<f64, String>),
    PositionSideChanged(PositionSide),
    OrderTypeChanged(OrderType),
    MarginModeChanged(MarginMode),
    QuantityChanged(String),
    PriceChanged(String),
    FillBestPrice,
    StopLossChanged(String),
    TakeProfitChanged(String),
    LeverageChanged(String),
    AllocationChanged(u8),
    RiskPercentChanged(String),
    RewardRatioChanged(String),
    ChartSourceChanged(ChartSource),
    PlaceOrder(Side, bool),
    CancelOrder(String, u64, bool),
    CancelAllOrders,
    ClosePosition(usize),
    EditProtection(usize),
    EditStopLossChanged(String),
    EditTakeProfitChanged(String),
    CloseProtectionEditor,
    SaveProtection,
    ToggleAccountAnalytics,
    ConfirmAction,
    CancelAction,
    ActionCompleted(Result<String, String>),
    ChartLevelDragged(TradingLineId, f64, bool),
    EventDurationChanged(EventDuration),
    EventStakeChanged(String),
    OpenEventContract(bool),
    EventShowOpenChanged(bool),
    EventShowHistoryChanged(bool),
    EventTabChanged(bool),
    OverlayAllChartsChanged(bool),
}

/// Order-ticket inputs restored across restarts.
#[derive(Deserialize, Serialize)]
#[serde(default)]
struct Ticket {
    position_side: PositionSide,
    order_type: OrderType,
    margin_mode: MarginMode,
    quantity: String,
    price: String,
    stop_loss: String,
    take_profit: String,
    leverage: String,
    risk_percent: String,
    reward_ratio: String,
    reduce_only: bool,
    event_duration: EventDuration,
    event_stake: String,
}

impl Default for Ticket {
    fn default() -> Self {
        Self {
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            margin_mode: MarginMode::Cross,
            quantity: String::new(),
            price: String::new(),
            stop_loss: String::new(),
            take_profit: String::new(),
            leverage: "10".to_string(),
            risk_percent: "1".to_string(),
            reward_ratio: "2".to_string(),
            reduce_only: false,
            event_duration: EventDuration::default(),
            event_stake: "50".to_string(),
        }
    }
}

pub struct Trader {
    config: TradingConfig,
    api_key: String,
    secret_key: String,
    client: Option<BinanceClient>,
    snapshot: Option<AccountSnapshot>,
    chart_sources: Vec<ChartSource>,
    selected_chart: Option<uuid::Uuid>,
    side: Side,
    position_side: PositionSide,
    order_type: OrderType,
    margin_mode: MarginMode,
    quantity: String,
    price: String,
    stop_loss: String,
    take_profit: String,
    protection_editor: Option<ProtectionEditor>,
    dragged_protection: Option<(Option<f64>, Option<f64>)>,
    /// Popup text when `pending_action` came from a chart drag.
    chart_confirm: Option<String>,
    show_account_analytics: bool,
    leverage: String,
    simulation_balance: String,
    simulation_maker_fee: String,
    simulation_taker_fee: String,
    risk_percent: String,
    reward_ratio: String,
    reduce_only: bool,
    pending_action: Option<PendingAction>,
    status: String,
    busy: bool,
    healthy: bool,
    latency_ms: Option<u128>,
    liquidation_fee_symbol: Option<String>,
    request_started: Option<std::time::Instant>,
    last_poll: Option<std::time::Instant>,
    proxy: Option<exchange::proxy::Proxy>,
    event_duration: EventDuration,
    event_stake: String,
    simulation_revision: u64,
}

/// Bumped on every simulation save; panels holding an older copy reload.
static SIMULATION_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Trader {
    pub fn new(mut config: TradingConfig, proxy: Option<exchange::proxy::Proxy>) -> Self {
        if config.account_id != data::config::trading::SHARED_ACCOUNT_ID {
            data::config::trading::adopt_credentials(&config.account_id);
            config.account_id = data::config::trading::SHARED_ACCOUNT_ID.to_string();
        }
        if config.exchange == TradingExchange::BinanceTestnet && !config.testnet {
            config.exchange = TradingExchange::BinanceLive;
        }
        config.poll_interval_secs = config.poll_interval_secs.clamp(1, 60);
        config.recv_window_ms = config.recv_window_ms.clamp(1_000, 60_000);
        config.allocation_percent = config.allocation_percent.min(100);
        let credentials =
            data::config::trading::load_credentials(&config.account_id, config.testnet);
        let simulation_balance = config.simulated_balance.to_string();
        let simulation_maker_fee = format_decimal(config.simulated_maker_fee_pct);
        let simulation_taker_fee = format_decimal(config.simulated_taker_fee_pct);
        let ticket: Ticket = serde_json::from_value(config.ticket.clone()).unwrap_or_default();
        let mut trader = Self {
            api_key: credentials
                .as_ref()
                .map(|value| value.api_key.clone())
                .unwrap_or_default(),
            secret_key: credentials
                .as_ref()
                .map(|value| value.secret_key.clone())
                .unwrap_or_default(),
            config,
            client: None,
            snapshot: None,
            chart_sources: Vec::new(),
            selected_chart: None,
            side: if ticket.position_side == PositionSide::Long {
                Side::Buy
            } else {
                Side::Sell
            },
            position_side: ticket.position_side,
            order_type: ticket.order_type,
            margin_mode: ticket.margin_mode,
            quantity: ticket.quantity,
            price: ticket.price,
            stop_loss: ticket.stop_loss,
            take_profit: ticket.take_profit,
            protection_editor: None,
            dragged_protection: None,
            chart_confirm: None,
            show_account_analytics: false,
            leverage: ticket.leverage,
            simulation_balance,
            simulation_maker_fee,
            simulation_taker_fee,
            risk_percent: ticket.risk_percent,
            reward_ratio: ticket.reward_ratio,
            reduce_only: ticket.reduce_only,
            pending_action: None,
            status: tr("Enter Binance USD-M API credentials and connect.").to_string(),
            busy: false,
            healthy: false,
            latency_ms: None,
            liquidation_fee_symbol: None,
            request_started: None,
            last_poll: None,
            proxy,
            event_duration: ticket.event_duration,
            event_stake: ticket.event_stake,
            simulation_revision: SIMULATION_REVISION.load(std::sync::atomic::Ordering::Relaxed),
        };
        if trader.is_simulation() {
            trader.snapshot = Some(load_simulated_snapshot(
                &trader.config.account_id,
                trader.config.simulated_balance,
            ));
            trader.apply_simulated_fees(false);
            trader.healthy = true;
            trader.status = tr("Local simulation ready.").to_string();
        }
        trader
    }

    /// Config to persist, including the current order-ticket inputs.
    pub fn config(&self) -> TradingConfig {
        let ticket = Ticket {
            position_side: self.position_side,
            order_type: self.order_type,
            margin_mode: self.margin_mode,
            quantity: self.quantity.clone(),
            price: self.price.clone(),
            stop_loss: self.stop_loss.clone(),
            take_profit: self.take_profit.clone(),
            leverage: self.leverage.clone(),
            risk_percent: self.risk_percent.clone(),
            reward_ratio: self.reward_ratio.clone(),
            reduce_only: self.reduce_only,
            event_duration: self.event_duration,
            event_stake: self.event_stake.clone(),
        };
        TradingConfig {
            ticket: serde_json::to_value(ticket).unwrap_or_default(),
            ..self.config.clone()
        }
    }

    pub fn set_proxy(&mut self, proxy: Option<exchange::proxy::Proxy>) {
        self.proxy = proxy;
    }

    pub fn set_chart_sources(&mut self, sources: Vec<ChartSource>) {
        let sources = sources
            .into_iter()
            .filter(|source| self.is_simulation() || source.binance_usdm)
            .collect::<Vec<_>>();
        let selected_exists = self
            .selected_chart
            .is_some_and(|id| sources.iter().any(|source| source.pane_id == id));
        if !selected_exists {
            self.selected_chart = sources
                .iter()
                .find(|source| source.symbol == self.config.symbol)
                .or_else(|| sources.first())
                .map(|source| source.pane_id);
        }
        self.chart_sources = sources;
        self.settle_event_contracts();
        if let Some(source) = self.selected_source().cloned() {
            self.config.symbol = source.symbol;
            if let Some(price) = source.latest_price {
                self.update_simulated_market(price);
            }
        }
        self.recalculate_quantity_if_empty();
    }

    pub fn selected_chart(&self) -> Option<uuid::Uuid> {
        self.selected_chart
    }

    /// Panes that should carry this trader's overlay: every compatible chart when
    /// "All charts" is on, otherwise only the selected one.
    /// Whether a drag on chart `source` belongs to this panel: the selected
    /// chart, or any chart of the traded symbol when trades show on every chart.
    pub fn accepts_chart_drag(&self, source: uuid::Uuid) -> bool {
        self.config.chart_drag_orders
            && (self.selected_chart == Some(source)
                || (self.config.overlay_all_charts
                    && self.chart_sources.iter().any(|chart| {
                        chart.pane_id == source && chart.symbol == self.config.symbol
                    })))
    }

    pub fn overlay_targets(&self) -> Vec<(uuid::Uuid, Option<PositionOverlay>)> {
        if self.config.overlay_all_charts {
            self.chart_sources
                .iter()
                .map(|source| (source.pane_id, self.overlay_for(&source.symbol)))
                .collect()
        } else {
            self.selected_chart
                .map(|id| (id, self.overlay_for(&self.config.symbol)))
                .into_iter()
                .collect()
        }
    }

    fn chart_row(&self) -> Element<'_, Message> {
        row![
            text(tr("Chart")),
            pick_list(
                self.chart_sources.clone(),
                self.selected_source().cloned(),
                Message::ChartSourceChanged
            )
            .placeholder(tr("No compatible chart"))
            .width(Length::Fill),
            tooltip(
                checkbox(self.config.overlay_all_charts)
                    .label(tr("All"))
                    .on_toggle(Message::OverlayAllChartsChanged),
                Some(tr("Show trades on every active chart")),
                tooltip::Position::Bottom,
            ),
            icon_button(
                crate::style::LucideIcon::RefreshCw,
                tr("Refresh account and chart source"),
                Message::Refresh,
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    fn protection_levels(&self, symbol: &str) -> (Option<f64>, Option<f64>) {
        let Some(snapshot) = &self.snapshot else {
            return (None, None);
        };
        let stop_loss = snapshot
            .open_orders
            .iter()
            .find(|order| {
                order.symbol == symbol
                    && order.order_type.contains("STOP")
                    && !order.order_type.contains("TAKE_PROFIT")
            })
            .map(|order| order.stop_price)
            .filter(|price| *price > 0.0);
        let take_profit = snapshot
            .open_orders
            .iter()
            .find(|order| order.symbol == symbol && order.order_type.contains("TAKE_PROFIT"))
            .map(|order| order.stop_price)
            .filter(|price| *price > 0.0);
        (stop_loss, take_profit)
    }

    /// Chart overlay (position, orders, fills, event contracts) for `symbol`.
    pub fn overlay_for(&self, symbol: &str) -> Option<PositionOverlay> {
        let snapshot = self.snapshot.as_ref()?;
        let show_position = self.config.chart_show_position;
        let position = snapshot
            .positions
            .iter()
            .find(|position| position.symbol == symbol)
            .filter(|_| show_position);
        let (mut stop_loss, mut take_profit) = self.protection_levels(symbol);
        if let Some((preview_stop, preview_take)) = self
            .dragged_protection
            .filter(|_| symbol == self.config.symbol)
        {
            stop_loss = preview_stop;
            take_profit = preview_take;
        }
        if !show_position {
            (stop_loss, take_profit) = (None, None);
        }
        let orders = snapshot
            .open_orders
            .iter()
            .filter(|order| show_position && order.symbol == symbol && order.order_type == "LIMIT")
            .map(|order| TradingOrderOverlay {
                id: TradingLineId::LimitOrder {
                    order_id: order.order_id,
                },
                label: format!("{} LIMIT #{}", order.side.api(), order.order_id),
                price: order.price,
                side: order.side,
            })
            .collect::<Vec<_>>();
        let trade_history = snapshot
            .trade_history
            .iter()
            .filter(|trade| {
                self.config.chart_show_fills
                    && trade.symbol == symbol
                    && trade.price > 0.0
                    && (!self.config.chart_opening_fills_only || trade.realized_pnl == 0.0)
            })
            .map(|trade| TradeMarker {
                time_ms: trade.time_ms,
                price: trade.price,
                side: trade.side,
            })
            .collect::<Vec<_>>();
        let event_contracts = snapshot
            .event_contracts
            .iter()
            .filter(|contract| {
                contract.symbol == symbol
                    && if contract.is_open() {
                        self.config.event_show_open
                    } else {
                        self.config.event_show_history
                    }
            })
            .cloned()
            .collect::<Vec<_>>();
        if position.is_none()
            && orders.is_empty()
            && trade_history.is_empty()
            && event_contracts.is_empty()
        {
            return None;
        }
        Some(PositionOverlay {
            symbol: symbol.to_string(),
            is_long: position.is_none_or(Position::is_long),
            entry_price: position.map(|position| position.entry_price),
            break_even_price: position.map(|position| position.economic_break_even_price),
            price_with_fees: self.config.chart_price_with_fees,
            fill_badges: self.config.chart_fill_badges,
            event_badges: self.config.event_chart_badges,
            draggable: self.config.chart_drag_orders && symbol == self.config.symbol,
            confirm: self
                .pending_action
                .as_ref()
                .and(self.chart_confirm.clone())
                .filter(|_| symbol == self.config.symbol),
            liquidation_price: position.and_then(|position| {
                (position.liquidation_price > 0.0).then_some(position.liquidation_price)
            }),
            liquidation_estimated: self.is_simulation(),
            stop_loss,
            take_profit,
            stop_loss_pnl: position.zip(stop_loss).map(|(position, price)| {
                position.estimated_pnl_at(price, snapshot.commission.taker)
            }),
            take_profit_pnl: position.zip(take_profit).map(|(position, price)| {
                position.estimated_pnl_at(price, snapshot.commission.taker)
            }),
            unrealized_pnl: position.map_or(0.0, |position| position.unrealized_pnl),
            orders,
            trade_history,
            event_contracts,
        })
    }

    fn protection_orders(&self, symbol: &str) -> Vec<OpenOrder> {
        self.snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .open_orders
                    .iter()
                    .filter(|order| {
                        order.symbol == symbol
                            && (order.order_type.contains("STOP")
                                || order.order_type.contains("TAKE_PROFIT"))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn tick(&mut self, now: std::time::Instant) -> Task<Message> {
        // Another layout's panel traded on the shared account: pick up its file
        // before this panel's stale copy can overwrite it.
        let revision = SIMULATION_REVISION.load(std::sync::atomic::Ordering::Relaxed);
        if self.simulation_revision != revision {
            self.simulation_revision = revision;
            if self.is_simulation() {
                self.snapshot = Some(load_simulated_snapshot(
                    &self.config.account_id,
                    self.config.simulated_balance,
                ));
                self.apply_simulated_fees(false);
            }
        }
        if self.is_simulation()
            && self
                .selected_source()
                .is_some_and(|source| source.binance_usdm)
            && self.liquidation_fee_symbol.as_deref() != Some(&self.config.symbol)
        {
            let symbol = self.config.symbol.clone();
            self.liquidation_fee_symbol = Some(symbol.clone());
            let proxy = self.proxy.clone();
            return Task::perform(
                fetch_binance_liquidation_fee(symbol, proxy),
                Message::LiquidationFeeLoaded,
            );
        }
        if self.client.is_none() || self.busy {
            return Task::none();
        }
        let due = self.last_poll.is_none_or(|last| {
            now.duration_since(last)
                >= std::time::Duration::from_secs(u64::from(self.config.poll_interval_secs))
        });
        if due {
            self.last_poll = Some(now);
            Task::done(Message::Poll)
        } else {
            Task::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AccountLabelChanged(value) => self.config.account_label = value,
            Message::ExchangeChanged(exchange) => {
                self.config.exchange = exchange;
                self.config.testnet = exchange != TradingExchange::BinanceLive;
                self.client = None;
                self.pending_action = None;
                if self.is_simulation() {
                    self.snapshot = Some(load_simulated_snapshot(
                        &self.config.account_id,
                        self.config.simulated_balance,
                    ));
                    self.apply_simulated_fees(false);
                    self.healthy = true;
                    self.status = tr("Local simulation ready.").to_string();
                } else {
                    self.snapshot = None;
                    self.healthy = false;
                    let credentials = data::config::trading::load_credentials(
                        &self.config.account_id,
                        self.config.testnet,
                    );
                    self.api_key = credentials
                        .as_ref()
                        .map(|value| value.api_key.clone())
                        .unwrap_or_default();
                    self.secret_key = credentials
                        .as_ref()
                        .map(|value| value.secret_key.clone())
                        .unwrap_or_default();
                    self.status = format!("{exchange} selected.");
                }
                self.recalculate_quantity();
            }
            Message::SimulationBalanceChanged(value) => {
                self.simulation_balance = integer_input(value);
            }
            Message::SimulationMakerFeeChanged(value) => {
                if let Some(pct) = fee_input(&value) {
                    self.config.simulated_maker_fee_pct = pct;
                    self.apply_simulated_fees(true);
                }
                self.simulation_maker_fee = value;
            }
            Message::SimulationTakerFeeChanged(value) => {
                if let Some(pct) = fee_input(&value) {
                    self.config.simulated_taker_fee_pct = pct;
                    self.apply_simulated_fees(true);
                }
                self.simulation_taker_fee = value;
            }
            Message::ResetSimulation => {
                let Some(balance) = self
                    .simulation_balance
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                else {
                    self.status = tr("Simulation balance must be greater than zero.").to_string();
                    return Task::none();
                };
                self.config.simulated_balance = balance;
                self.snapshot = Some(simulated_snapshot(balance));
                self.apply_simulated_fees(false);
                self.healthy = true;
                self.persist_simulation();
                self.recalculate_quantity();
                self.status = tr("Simulation account reset.").to_string();
            }
            Message::ApiKeyChanged(value) => self.api_key = value,
            Message::SecretKeyChanged(value) => self.secret_key = value,
            Message::ConfirmOrdersChanged(value) => self.config.confirm_orders = value,
            Message::ChartPriceWithFeesChanged(value) => {
                self.config.chart_price_with_fees = value;
            }
            Message::ChartShowPositionChanged(value) => self.config.chart_show_position = value,
            Message::ChartShowFillsChanged(value) => self.config.chart_show_fills = value,
            Message::ChartFillBadgesChanged(value) => self.config.chart_fill_badges = value,
            Message::EventChartBadgesChanged(value) => self.config.event_chart_badges = value,
            Message::ChartOpeningFillsOnlyChanged(value) => {
                self.config.chart_opening_fills_only = value;
            }
            Message::ChartDragOrdersChanged(value) => self.config.chart_drag_orders = value,
            Message::ChartDragConfirmChanged(value) => self.config.chart_drag_confirm = value,
            Message::SaveCredentials => {
                self.status = match data::config::trading::save_credentials(
                    &self.config.account_id,
                    self.config.testnet,
                    &self.api_key,
                    &self.secret_key,
                ) {
                    Ok(()) => tr("Trading credentials saved to the system keychain.").to_string(),
                    Err(error) => error,
                };
            }
            Message::Connect => {
                if self.is_simulation() {
                    self.status = tr("Local simulation is already connected.").to_string();
                    return Task::none();
                }
                if self.busy {
                    return Task::none();
                }
                let credentials = TradingCredentials {
                    api_key: self.api_key.trim().to_string(),
                    secret_key: self.secret_key.trim().to_string(),
                };
                if credentials.api_key.is_empty() || credentials.secret_key.is_empty() {
                    self.status = tr("API key and secret key are required.").to_string();
                    return Task::none();
                }
                if let Err(error) = data::config::trading::save_credentials(
                    &self.config.account_id,
                    self.config.testnet,
                    &credentials.api_key,
                    &credentials.secret_key,
                ) {
                    self.status = error;
                    return Task::none();
                }
                let client = match BinanceClient::new_with_proxy(
                    &self.config,
                    credentials,
                    self.proxy.as_ref(),
                ) {
                    Ok(client) => client,
                    Err(error) => {
                        self.status = error;
                        return Task::none();
                    }
                };
                let symbol = self.config.symbol.clone();
                self.busy = true;
                self.request_started = Some(std::time::Instant::now());
                self.status = tr("Connecting to Binance...").to_string();
                return Task::perform(
                    async move { client.connect(&symbol).await },
                    Message::Connected,
                );
            }
            Message::Connected(result) => {
                self.busy = false;
                self.finish_request_timing();
                match result {
                    Ok((client, snapshot)) => {
                        self.client = Some(client);
                        self.snapshot = Some(snapshot);
                        self.recalculate_quantity();
                        self.healthy = true;
                        self.last_poll = Some(std::time::Instant::now());
                        self.status = format!(
                            "Connected to Binance USD-M {}.",
                            if self.config.testnet {
                                "testnet"
                            } else {
                                "live"
                            }
                        );
                    }
                    Err(error) => {
                        self.client = None;
                        self.healthy = false;
                        self.status = error;
                    }
                }
            }
            Message::Disconnect => {
                self.client = None;
                self.snapshot = None;
                self.healthy = false;
                self.pending_action = None;
                self.status = tr("Disconnected from Binance.").to_string();
            }
            refresh @ (Message::Refresh | Message::Poll) => {
                let Some(client) = self.client.clone() else {
                    return Task::none();
                };
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.request_started = Some(std::time::Instant::now());
                let symbol = self.config.symbol.clone();
                let fast = matches!(refresh, Message::Poll);
                let hedge_mode = self.snapshot.as_ref().is_some_and(|value| value.hedge_mode);
                let commission = self
                    .snapshot
                    .as_ref()
                    .map(|value| value.commission)
                    .unwrap_or_default();
                return Task::perform(
                    async move {
                        if fast {
                            client
                                .account_snapshot_fast(&symbol, hedge_mode, commission)
                                .await
                        } else {
                            client.account_snapshot(&symbol).await
                        }
                    },
                    Message::SnapshotLoaded,
                );
            }
            Message::SnapshotLoaded(result) => {
                self.busy = false;
                self.finish_request_timing();
                match result {
                    Ok(mut snapshot) => {
                        if let Some(previous) = &self.snapshot {
                            preserve_position_costs(previous, &mut snapshot);
                            if snapshot.trade_history.is_empty() {
                                snapshot.trade_history = previous.trade_history.clone();
                            }
                        }
                        self.snapshot = Some(snapshot);
                        self.dragged_protection = None;
                        self.healthy = true;
                        self.recalculate_quantity_if_empty();
                        self.last_poll = Some(std::time::Instant::now());
                        self.status = tr("Account synchronized.").to_string();
                    }
                    Err(error) => {
                        self.healthy = false;
                        self.status = error;
                    }
                }
            }
            Message::LiquidationFeeLoaded(result) => match result {
                Ok(rate) => {
                    if let Some(snapshot) = &mut self.snapshot {
                        snapshot.liquidation_fee_rate = rate;
                    }
                    if let Some(price) = self
                        .selected_source()
                        .and_then(|source| source.latest_price)
                    {
                        self.update_simulated_market(price);
                    }
                }
                Err(error) => log::warn!("Failed to load Binance liquidation fee: {error}"),
            },
            Message::PositionSideChanged(value) => {
                self.position_side = value;
                self.side = if value == PositionSide::Long {
                    Side::Buy
                } else {
                    Side::Sell
                };
                self.calculate_risk_levels();
            }
            Message::OrderTypeChanged(value) => {
                self.order_type = value;
                self.calculate_risk_levels();
            }
            Message::MarginModeChanged(value) => self.margin_mode = value,
            Message::QuantityChanged(value) => self.quantity = numeric_input(value),
            Message::PriceChanged(value) => {
                self.price = numeric_input(value);
                self.calculate_risk_levels();
            }
            Message::FillBestPrice => {
                if let Some(source) = self.selected_source() {
                    let price = match self.side {
                        Side::Buy => source.best_bid,
                        Side::Sell => source.best_ask,
                    }
                    .or(source.latest_price);
                    if let Some(price) = price {
                        self.price = format_decimal(round_to_step(price, source.min_tick));
                        self.calculate_risk_levels();
                    }
                }
            }
            Message::StopLossChanged(value) => self.stop_loss = numeric_input(value),
            Message::TakeProfitChanged(value) => self.take_profit = numeric_input(value),
            Message::LeverageChanged(value) => {
                self.leverage = integer_input(value);
                self.recalculate_quantity();
            }
            Message::AllocationChanged(value) => {
                self.config.allocation_percent = value.min(100);
                self.recalculate_quantity();
            }
            Message::RiskPercentChanged(value) => {
                self.risk_percent = numeric_input(value);
                self.calculate_risk_levels();
            }
            Message::RewardRatioChanged(value) => {
                self.reward_ratio = numeric_input(value);
                self.calculate_risk_levels();
            }
            Message::ChartSourceChanged(source) => {
                self.selected_chart = Some(source.pane_id);
                self.config.symbol = source.symbol;
                self.liquidation_fee_symbol = None;
                self.recalculate_quantity();
                self.calculate_risk_levels();
                if self.client.is_some() {
                    return Task::done(Message::Refresh);
                }
            }
            Message::PlaceOrder(side, reduce_only) => {
                self.side = side;
                self.reduce_only = reduce_only;
                self.calculate_risk_levels();
                let request = match self.order_request() {
                    Ok(request) => request,
                    Err(error) => {
                        self.status = error;
                        return Task::none();
                    }
                };
                let Some(leverage) = self
                    .leverage
                    .parse::<u16>()
                    .ok()
                    .filter(|value| (1..=125).contains(value))
                else {
                    self.status = tr("Leverage must be between 1 and 125.").to_string();
                    return Task::none();
                };
                return self.queue_or_execute(PendingAction::Place {
                    request,
                    leverage,
                    margin_mode: self.margin_mode,
                });
            }
            Message::CancelOrder(symbol, order_id, is_algo) => {
                return self.queue_or_execute(PendingAction::Cancel {
                    symbol,
                    order_id,
                    is_algo,
                });
            }
            Message::CancelAllOrders => {
                return self.queue_or_execute(PendingAction::CancelAll {
                    symbol: self.config.symbol.clone(),
                });
            }
            Message::ClosePosition(index) => {
                let Some(position) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.positions.get(index))
                    .cloned()
                else {
                    return Task::none();
                };
                return self.queue_or_execute(PendingAction::Close(position));
            }
            Message::EditProtection(index) => {
                let Some(position) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.positions.get(index))
                    .cloned()
                else {
                    return Task::none();
                };
                let (stop_loss, take_profit) = self.protection_levels(&position.symbol);
                self.protection_editor = Some(ProtectionEditor {
                    symbol: position.symbol,
                    stop_loss: stop_loss.map(format_price_2).unwrap_or_default(),
                    take_profit: take_profit.map(format_price_2).unwrap_or_default(),
                });
            }
            Message::EditStopLossChanged(value) => {
                if let Some(editor) = &mut self.protection_editor {
                    editor.stop_loss = numeric_input(value);
                }
            }
            Message::EditTakeProfitChanged(value) => {
                if let Some(editor) = &mut self.protection_editor {
                    editor.take_profit = numeric_input(value);
                }
            }
            Message::CloseProtectionEditor => self.protection_editor = None,
            Message::ToggleAccountAnalytics => {
                self.show_account_analytics = !self.show_account_analytics;
            }
            Message::SaveProtection => {
                let Some(editor) = self.protection_editor.clone() else {
                    return Task::none();
                };
                let Some(position) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        snapshot
                            .positions
                            .iter()
                            .find(|position| position.symbol == editor.symbol)
                    })
                    .cloned()
                else {
                    self.status = tr("The position is no longer open.").to_string();
                    self.protection_editor = None;
                    return Task::none();
                };
                let stop_loss = optional_price(&editor.stop_loss);
                let take_profit = optional_price(&editor.take_profit);
                if stop_loss.is_none() && take_profit.is_none() {
                    self.status = tr("Enter a stop loss or take profit price first.").to_string();
                    return Task::none();
                }
                self.protection_editor = None;
                return self.queue_or_execute(PendingAction::Protect {
                    existing: self.protection_orders(&position.symbol),
                    position,
                    stop_loss,
                    take_profit,
                });
            }
            Message::ConfirmAction => {
                let Some(action) = self.pending_action.take() else {
                    return Task::none();
                };
                return self.execute(action);
            }
            Message::CancelAction => {
                self.pending_action = None;
                // A cancelled chart drag snaps the line back.
                self.dragged_protection = None;
            }
            Message::ActionCompleted(result) => {
                self.busy = false;
                self.dragged_protection = None;
                self.status = match result {
                    Ok(message) => message,
                    Err(error) => error,
                };
                if self.client.is_some() {
                    return Task::done(Message::Refresh);
                }
            }
            Message::EventDurationChanged(duration) => self.event_duration = duration,
            Message::EventStakeChanged(value) => self.event_stake = integer_input(value),
            Message::OpenEventContract(up) => self.open_event_contract(up),
            Message::EventShowOpenChanged(show) => self.config.event_show_open = show,
            Message::EventShowHistoryChanged(show) => self.config.event_show_history = show,
            Message::EventTabChanged(event_tab) => self.config.event_tab = event_tab,
            Message::OverlayAllChartsChanged(all) => self.config.overlay_all_charts = all,
            Message::ChartLevelDragged(id, price, finished) => {
                let price = self
                    .selected_source()
                    .map_or(price, |source| round_to_step(price, source.min_tick));
                match id {
                    TradingLineId::StopLoss | TradingLineId::TakeProfit => {
                        let (mut stop_loss, mut take_profit) =
                            self.protection_levels(&self.config.symbol);
                        match id {
                            TradingLineId::StopLoss => stop_loss = Some(price),
                            TradingLineId::TakeProfit => take_profit = Some(price),
                            TradingLineId::LimitOrder { .. } => unreachable!(),
                        }
                        self.dragged_protection = Some((stop_loss, take_profit));
                    }
                    TradingLineId::LimitOrder { order_id } => {
                        if let Some(order) = self.snapshot.as_mut().and_then(|snapshot| {
                            snapshot
                                .open_orders
                                .iter_mut()
                                .find(|order| order.order_id == order_id)
                        }) {
                            order.price = price;
                        }
                    }
                }
                if !finished {
                    return Task::none();
                }
                let confirm = self.config.chart_drag_confirm;
                let queued = confirm || self.config.exchange == TradingExchange::BinanceLive;
                let summary = match id {
                    TradingLineId::StopLoss | TradingLineId::TakeProfit => {
                        let tag = if id == TradingLineId::StopLoss {
                            "SL"
                        } else {
                            "TP"
                        };
                        let pnl = self.snapshot.as_ref().and_then(|snapshot| {
                            snapshot
                                .positions
                                .iter()
                                .find(|position| position.symbol == self.config.symbol)
                                .map(|position| {
                                    position.estimated_pnl_at(price, snapshot.commission.taker)
                                })
                        });
                        match pnl {
                            Some(pnl) => format!(
                                "{tag} → {}   {} {pnl:+.2}",
                                format_decimal(price),
                                tr("Est. PnL")
                            ),
                            None => format!("{tag} → {}", format_decimal(price)),
                        }
                    }
                    TradingLineId::LimitOrder { .. } => {
                        format!("{} → {}", tr("Move limit order"), format_decimal(price))
                    }
                };
                let task = match id {
                    TradingLineId::StopLoss | TradingLineId::TakeProfit => self
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot
                                .positions
                                .iter()
                                .find(|position| position.symbol == self.config.symbol)
                        })
                        .cloned()
                        .map(|position| {
                            self.queue_or_execute_with(
                                PendingAction::Protect {
                                    existing: self.protection_orders(&position.symbol),
                                    position,
                                    stop_loss: self.dragged_protection.and_then(|levels| levels.0),
                                    take_profit: self
                                        .dragged_protection
                                        .and_then(|levels| levels.1),
                                },
                                confirm,
                            )
                        })
                        .unwrap_or_else(Task::none),
                    TradingLineId::LimitOrder { order_id } => self
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot
                                .open_orders
                                .iter()
                                .find(|order| order.order_id == order_id)
                        })
                        .cloned()
                        .map(|order| {
                            self.queue_or_execute_with(
                                PendingAction::ModifyLimit { order, price },
                                confirm,
                            )
                        })
                        .unwrap_or_else(Task::none),
                };
                if queued && self.pending_action.is_some() {
                    self.chart_confirm = Some(summary);
                }
                return task;
            }
        }
        Task::none()
    }

    fn selected_source(&self) -> Option<&ChartSource> {
        let selected = self.selected_chart?;
        self.chart_sources
            .iter()
            .find(|source| source.pane_id == selected)
    }

    fn is_simulation(&self) -> bool {
        self.config.exchange == TradingExchange::Simulation
    }

    fn connected(&self) -> bool {
        if self.is_simulation() {
            self.snapshot.is_some()
        } else {
            self.client.is_some()
        }
    }

    /// Simulated account uses the fee rates from settings.
    fn apply_simulated_fees(&mut self, persist: bool) {
        if !self.is_simulation() {
            return;
        }
        let rate = CommissionRate {
            maker: self.config.simulated_maker_fee_pct / 100.0,
            taker: self.config.simulated_taker_fee_pct / 100.0,
        };
        if let Some(snapshot) = self.snapshot.as_mut() {
            let changed = (snapshot.commission.maker - rate.maker).abs() > f64::EPSILON
                || (snapshot.commission.taker - rate.taker).abs() > f64::EPSILON;
            snapshot.commission = rate;
            if changed && persist {
                self.persist_simulation();
            }
        }
    }

    fn persist_simulation(&mut self) {
        if self.is_simulation()
            && let Some(snapshot) = &self.snapshot
        {
            if let Err(error) = save_simulated_snapshot(&self.config.account_id, snapshot) {
                log::error!("Failed to persist simulation account: {error}");
            }
            self.simulation_revision =
                SIMULATION_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        }
    }

    /// Settles expired event contracts of every charted symbol and credits wins.
    fn settle_event_contracts(&mut self) {
        if !self.is_simulation() {
            return;
        }
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let now = local_time_ms();
        let mut settled = false;
        for source in &self.chart_sources {
            let Some(price) = source.latest_price else {
                continue;
            };
            if let Some(credit) =
                event::settle_due(&mut snapshot.event_contracts, &source.symbol, price, now)
            {
                settled = true;
                if let Some(usdt) = snapshot.balances.iter_mut().find(|b| b.asset == "USDT") {
                    usdt.wallet += credit;
                    usdt.available += credit;
                }
            }
        }
        if settled {
            self.persist_simulation();
        }
    }

    fn open_event_contract(&mut self, up: bool) {
        if !self.is_simulation() {
            self.status = tr("Event contracts are simulation only.").to_string();
            return;
        }
        let price = self
            .selected_source()
            .and_then(|source| source.latest_price);
        let symbol = self.config.symbol.clone();
        let duration = self.event_duration;
        let stake = self.event_stake.parse::<f64>().unwrap_or(0.0);
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let available = snapshot
            .balances
            .iter()
            .find(|b| b.asset == "USDT")
            .map_or(0.0, |b| b.available);
        match event::open(
            &snapshot.event_contracts,
            &symbol,
            up,
            duration,
            stake,
            price.unwrap_or(0.0),
            local_time_ms(),
            available,
        ) {
            Ok(contract) => {
                if let Some(usdt) = snapshot.balances.iter_mut().find(|b| b.asset == "USDT") {
                    usdt.wallet -= stake;
                    usdt.available -= stake;
                }
                self.status = format!(
                    "Event {} {} {stake:.0} USDT @ {}",
                    if up { "UP" } else { "DOWN" },
                    duration,
                    format_decimal(contract.open_price)
                );
                snapshot.event_contracts.push(contract);
                self.persist_simulation();
            }
            Err(error) => self.status = error,
        }
    }

    fn update_simulated_market(&mut self, price: f64) {
        if !self.is_simulation() {
            return;
        }
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let leverage = self.leverage.parse().unwrap_or(1);
        let filled = match_simulated_orders(
            snapshot,
            &self.config.symbol,
            price,
            leverage,
            self.margin_mode,
        );
        refresh_simulated_account(snapshot, &self.config.symbol, price);
        if filled {
            self.persist_simulation();
        }
    }

    fn recalculate_quantity(&mut self) {
        let Some(price) = self
            .selected_source()
            .and_then(|source| source.latest_price)
        else {
            return;
        };
        let Some(available) = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .balances
                .iter()
                .find(|balance| balance.asset == "USDT")
                .map(|balance| balance.available)
        }) else {
            return;
        };
        let Some(leverage) = self.leverage.parse::<u16>().ok().filter(|value| *value > 0) else {
            return;
        };
        let step = self.selected_source().map_or(0.0, |source| source.min_qty);
        if let Some(quantity) = quantity_from_allocation(
            available,
            self.config.allocation_percent,
            leverage,
            price,
            step,
        ) {
            self.quantity = format_decimal(quantity);
        }
    }

    fn recalculate_quantity_if_empty(&mut self) {
        if self.quantity.trim().is_empty() {
            self.recalculate_quantity();
        }
    }

    fn finish_request_timing(&mut self) {
        self.latency_ms = self
            .request_started
            .take()
            .map(|started| started.elapsed().as_millis());
    }

    fn order_request(&self) -> Result<OrderRequest, String> {
        let quantity = parse_input(&self.quantity, tr("Quantity"))?;
        let price = if self.order_type == OrderType::Limit {
            Some(parse_input(&self.price, tr("Limit price"))?)
        } else {
            None
        };
        let position_side = if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.hedge_mode)
        {
            self.position_side
        } else {
            PositionSide::Both
        };
        let mut request = OrderRequest {
            symbol: self.config.symbol.clone(),
            side: self.side,
            position_side,
            order_type: self.order_type,
            quantity,
            price,
            reduce_only: self.reduce_only,
            stop_loss: if self.reduce_only {
                None
            } else {
                optional_price(&self.stop_loss)
            },
            take_profit: if self.reduce_only {
                None
            } else {
                optional_price(&self.take_profit)
            },
        };
        if let Some(source) = self
            .chart_sources
            .iter()
            .find(|source| source.symbol == request.symbol)
        {
            request.quantity = floor_to_step(request.quantity, source.min_qty);
            request.price = request
                .price
                .map(|value| round_to_step(value, source.min_tick));
            request.stop_loss = request
                .stop_loss
                .map(|value| round_to_step(value, source.min_tick));
            request.take_profit = request
                .take_profit
                .map(|value| round_to_step(value, source.min_tick));
        }
        request.validate()?;
        Ok(request)
    }

    fn queue_or_execute(&mut self, action: PendingAction) -> Task<Message> {
        self.chart_confirm = None;
        self.queue_or_execute_with(action, self.config.confirm_orders)
    }

    /// Chart drags use their own confirmation switch; live funds always confirm.
    fn queue_or_execute_with(&mut self, action: PendingAction, confirm: bool) -> Task<Message> {
        if self.client.is_none() && !self.is_simulation() {
            self.status = tr("Connect a Binance account first.").to_string();
            self.dragged_protection = None;
            return Task::none();
        }
        if confirm || self.config.exchange == TradingExchange::BinanceLive {
            self.pending_action = Some(action);
            Task::none()
        } else {
            self.execute(action)
        }
    }

    fn execute(&mut self, action: PendingAction) -> Task<Message> {
        if self.is_simulation() {
            self.status = match self.execute_simulated(action) {
                Ok(message) => message,
                Err(error) => error,
            };
            self.dragged_protection = None;
            self.persist_simulation();
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        self.busy = true;
        self.status = tr("Sending trading request...").to_string();
        Task::perform(
            async move {
                match action {
                    PendingAction::Place {
                        request,
                        leverage,
                        margin_mode,
                    } => {
                        let reference = match request.price {
                            Some(price) => price,
                            None => client.mark_price(&request.symbol).await?,
                        };
                        validate_protection(
                            request.side == Side::Buy,
                            reference,
                            request.stop_loss,
                            request.take_profit,
                        )?;
                        client.set_margin_mode(&request.symbol, margin_mode).await?;
                        client.set_leverage(&request.symbol, leverage).await?;
                        let order = client.place_order(request).await?;
                        Ok(format!(
                            "Order {} accepted for {} ({}).",
                            order.order_id, order.symbol, order.status
                        ))
                    }
                    PendingAction::Cancel {
                        symbol,
                        order_id,
                        is_algo,
                    } => {
                        client.cancel_order(&symbol, order_id, is_algo).await?;
                        Ok(format!("Order {order_id} canceled."))
                    }
                    PendingAction::CancelAll { symbol } => {
                        client.cancel_all_orders(&symbol).await?;
                        Ok(format!("All open {symbol} orders canceled."))
                    }
                    PendingAction::Close(position) => {
                        let order = client.close_position(&position).await?;
                        Ok(format!("Close order {} accepted.", order.order_id))
                    }
                    PendingAction::Protect {
                        position,
                        stop_loss,
                        take_profit,
                        existing,
                    } => {
                        let entry_side = if position.is_long() {
                            Side::Buy
                        } else {
                            Side::Sell
                        };
                        if let Some(price) = stop_loss {
                            validate_protection(
                                position.is_long(),
                                position.mark_price,
                                Some(price),
                                None,
                            )?;
                            client
                                .place_protective_order(
                                    &position.symbol,
                                    entry_side,
                                    position.side,
                                    "STOP_MARKET",
                                    price,
                                )
                                .await?;
                        }
                        if let Some(price) = take_profit {
                            validate_protection(
                                position.is_long(),
                                position.mark_price,
                                None,
                                Some(price),
                            )?;
                            client
                                .place_protective_order(
                                    &position.symbol,
                                    entry_side,
                                    position.side,
                                    "TAKE_PROFIT_MARKET",
                                    price,
                                )
                                .await?;
                        }
                        for order in existing {
                            client
                                .cancel_order(&order.symbol, order.order_id, order.is_algo)
                                .await?;
                        }
                        Ok(tr("Position protection orders accepted.").to_string())
                    }
                    PendingAction::ModifyLimit { order, price } => {
                        client.modify_limit_order(&order, price).await?;
                        Ok(format!(
                            "Order {} price changed to {}.",
                            order.order_id,
                            format_decimal(price)
                        ))
                    }
                }
            },
            Message::ActionCompleted,
        )
    }

    fn execute_simulated(&mut self, action: PendingAction) -> Result<String, String> {
        let market = self
            .selected_source()
            .and_then(|source| source.latest_price);
        let snapshot = self
            .snapshot
            .as_mut()
            .ok_or_else(|| tr("Reset the simulation account first.").to_string())?;
        match action {
            PendingAction::Place {
                request,
                leverage,
                margin_mode,
            } => {
                let market = market.ok_or_else(|| {
                    tr("The linked chart has no current market price.").to_string()
                })?;
                let limit_waits = request.price.is_some_and(|price| match request.side {
                    Side::Buy => price < market,
                    Side::Sell => price > market,
                });
                if limit_waits {
                    let order_id = simulation_order_id(snapshot);
                    snapshot.open_orders.push(OpenOrder {
                        symbol: request.symbol.clone(),
                        order_id,
                        side: request.side,
                        order_type: "LIMIT".to_string(),
                        price: request.price.unwrap_or(market),
                        stop_price: 0.0,
                        quantity: request.quantity,
                        reduce_only: request.reduce_only,
                        is_algo: false,
                    });
                    return Ok(format!("Simulation limit order {order_id} accepted."));
                }
                let fill_price = request.price.unwrap_or(market);
                validate_protection(
                    request.side == Side::Buy,
                    fill_price,
                    request.stop_loss,
                    request.take_profit,
                )?;
                simulated_fill(
                    snapshot,
                    &request.symbol,
                    request.side,
                    request.quantity,
                    fill_price,
                    leverage,
                    margin_mode,
                    request.reduce_only,
                )?;
                add_simulated_protection(
                    snapshot,
                    &request.symbol,
                    request.side,
                    request.quantity,
                    request.stop_loss,
                    request.take_profit,
                );
                Ok(format!(
                    "Simulation order filled at {}.",
                    format_decimal(fill_price)
                ))
            }
            PendingAction::Cancel { order_id, .. } => {
                let before = snapshot.open_orders.len();
                snapshot
                    .open_orders
                    .retain(|order| order.order_id != order_id);
                (snapshot.open_orders.len() < before)
                    .then(|| format!("Simulation order {order_id} canceled."))
                    .ok_or_else(|| "Simulation order was not found.".to_string())
            }
            PendingAction::CancelAll { symbol } => {
                snapshot.open_orders.retain(|order| order.symbol != symbol);
                Ok(format!("All simulated {symbol} orders canceled."))
            }
            PendingAction::Close(position) => {
                let market = market.unwrap_or(position.mark_price);
                simulated_fill(
                    snapshot,
                    &position.symbol,
                    if position.is_long() {
                        Side::Sell
                    } else {
                        Side::Buy
                    },
                    position.quantity.abs(),
                    market,
                    position.leverage,
                    position.margin_mode,
                    true,
                )?;
                snapshot
                    .open_orders
                    .retain(|order| order.symbol != position.symbol);
                Ok(format!("Simulated {} position closed.", position.symbol))
            }
            PendingAction::Protect {
                position,
                stop_loss,
                take_profit,
                existing: _,
            } => {
                let market = market.unwrap_or(position.mark_price);
                validate_protection(position.is_long(), market, stop_loss, take_profit)?;
                add_simulated_protection(
                    snapshot,
                    &position.symbol,
                    if position.is_long() {
                        Side::Buy
                    } else {
                        Side::Sell
                    },
                    position.quantity.abs(),
                    stop_loss,
                    take_profit,
                );
                Ok(tr("Simulation protection updated.").to_string())
            }
            PendingAction::ModifyLimit { order, price } => {
                let target = snapshot
                    .open_orders
                    .iter_mut()
                    .find(|candidate| candidate.order_id == order.order_id)
                    .ok_or_else(|| "Simulation order was not found.".to_string())?;
                target.price = price;
                Ok(format!("Simulation order {} moved.", order.order_id))
            }
        }
    }

    pub fn settings_view(&self) -> Element<'_, Message> {
        let connected = self.connected();
        let account_name: Element<'_, Message> = if self.is_simulation() {
            text(tr("Local simulation account")).into()
        } else {
            labeled_input(
                tr("Account name"),
                "Primary",
                &self.config.account_label,
                Message::AccountLabelChanged,
            )
        };
        let connection: Element<'_, Message> = if self.is_simulation() {
            column![
                labeled_input(
                    tr("Initial balance (USDT)"),
                    "10000",
                    &self.simulation_balance,
                    Message::SimulationBalanceChanged,
                ),
                row![
                    labeled_input(
                        tr("Maker fee %"),
                        "0.02",
                        &self.simulation_maker_fee,
                        Message::SimulationMakerFeeChanged,
                    ),
                    labeled_input(
                        tr("Taker fee %"),
                        "0.04",
                        &self.simulation_taker_fee,
                        Message::SimulationTakerFeeChanged,
                    ),
                ]
                .spacing(8),
                button(text(tr("Reset simulation"))).on_press(Message::ResetSimulation),
            ]
            .spacing(6)
            .into()
        } else {
            column![
                column![
                    text(tr("API key")).size(crate::style::text_size::SMALL),
                    text_input("Key", &self.api_key)
                        .secure(true)
                        .on_input(Message::ApiKeyChanged),
                ]
                .spacing(2),
                column![
                    text(tr("Secret key")).size(crate::style::text_size::SMALL),
                    text_input("Secret", &self.secret_key)
                        .secure(true)
                        .on_input(Message::SecretKeyChanged),
                ]
                .spacing(2),
                row![
                    button(text(tr("Save"))).on_press(Message::SaveCredentials),
                    if connected {
                        button(text(tr("Disconnect"))).on_press(Message::Disconnect)
                    } else if self.busy {
                        button(text(tr("Connecting")))
                    } else {
                        button(text(tr("Connect"))).on_press(Message::Connect)
                    },
                ]
                .spacing(6),
            ]
            .spacing(6)
            .into()
        };
        container(crate::style::thin_scrollable(
            column![
                row![
                    text(tr("Exchange")).width(Length::Fill),
                    pick_list(
                        TradingExchange::ALL,
                        Some(self.config.exchange),
                        Message::ExchangeChanged,
                    ),
                ]
                .align_y(Alignment::Center),
                account_name,
                row![
                    checkbox(self.config.confirm_orders)
                        .label(tr("Confirm orders"))
                        .on_toggle(Message::ConfirmOrdersChanged),
                    text(if self.config.exchange == TradingExchange::BinanceLive {
                        "LIVE FUNDS · BETA"
                    } else if self.is_simulation() {
                        "LOCAL"
                    } else {
                        "TESTNET"
                    }),
                ]
                .spacing(10),
                column![
                    text(tr(
                        "Live trading is in beta and not fully tested yet. Use small sizes."
                    ))
                    .size(crate::style::text_size::SMALL)
                    .style(|theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(theme.extended_palette().danger.base.color),
                    })
                ]
                .height(if self.config.exchange == TradingExchange::BinanceLive {
                    Length::Shrink
                } else {
                    Length::Fixed(0.0)
                }),
                row![
                    text(tr("Chart position line")).width(Length::Fill),
                    pick_list(
                        [tr("Entry price"), tr("Break-even (incl. fees)")],
                        Some(if self.config.chart_price_with_fees {
                            tr("Break-even (incl. fees)")
                        } else {
                            tr("Entry price")
                        }),
                        |choice| Message::ChartPriceWithFeesChanged(
                            choice == tr("Break-even (incl. fees)")
                        ),
                    ),
                ]
                .align_y(Alignment::Center),
                row![
                    text(tr("Show on chart")).width(Length::Fill),
                    checkbox(self.config.chart_show_position)
                        .label(tr("Position & orders"))
                        .on_toggle(Message::ChartShowPositionChanged),
                    checkbox(self.config.chart_show_fills)
                        .label(tr("Fills"))
                        .on_toggle(Message::ChartShowFillsChanged),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                row![
                    text(tr("Futures fill markers")).width(Length::Fill),
                    pick_list(
                        [tr("B/S badge on candle"), tr("Arrow at fill price")],
                        Some(if self.config.chart_fill_badges {
                            tr("B/S badge on candle")
                        } else {
                            tr("Arrow at fill price")
                        }),
                        |choice| Message::ChartFillBadgesChanged(
                            choice == tr("B/S badge on candle")
                        ),
                    ),
                ]
                .align_y(Alignment::Center),
                checkbox(self.config.chart_opening_fills_only)
                    .label(tr("Opening fills only"))
                    .on_toggle(Message::ChartOpeningFillsOnlyChanged),
                row![
                    text(tr("Event contract markers")).width(Length::Fill),
                    pick_list(
                        [tr("W/L badge on candle"), tr("Line with result")],
                        Some(if self.config.event_chart_badges {
                            tr("W/L badge on candle")
                        } else {
                            tr("Line with result")
                        }),
                        |choice| Message::EventChartBadgesChanged(
                            choice == tr("W/L badge on candle")
                        ),
                    ),
                ]
                .align_y(Alignment::Center),
                row![
                    checkbox(self.config.chart_drag_orders)
                        .label(tr("Drag TP/SL and orders on chart"))
                        .on_toggle(Message::ChartDragOrdersChanged),
                    checkbox(self.config.chart_drag_confirm)
                        .label(tr("Confirm chart edits"))
                        .on_toggle_maybe(
                            self.config
                                .chart_drag_orders
                                .then_some(Message::ChartDragConfirmChanged)
                        ),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                connection,
                text(&self.status).size(crate::style::text_size::SMALL),
            ]
            .spacing(8)
            .padding(padding::all(12)),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub fn view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        let connected = self.connected();
        let selected_chart = self
            .selected_chart
            .and_then(|id| {
                self.chart_sources
                    .iter()
                    .find(|source| source.pane_id == id)
            })
            .cloned();
        let account_header: Element<'_, Message> = self.snapshot.as_ref().map_or_else(
            || row![text(tr("No account data"))].into(),
            |snapshot| {
                let usdt = snapshot
                    .balances
                    .iter()
                    .find(|balance| balance.asset == "USDT");
                column![
                    text(usdt.map_or_else(
                        || "USDT --".to_string(),
                        |balance| format!("USDT {:.2}", balance.wallet)
                    )),
                    text(usdt.map_or_else(
                        || "Available --".to_string(),
                        |balance| format!("Available {:.2}", balance.available)
                    )),
                    text(format!(
                        "{} positions | {} orders",
                        snapshot.positions.len(),
                        snapshot.open_orders.len()
                    )),
                    text(format!(
                        "Fees M {:.4}% / T {:.4}%",
                        snapshot.commission.maker * 100.0,
                        snapshot.commission.taker * 100.0
                    )),
                ]
                .spacing(3)
                .into()
            },
        );

        let market_price = selected_chart
            .as_ref()
            .and_then(|source| source.latest_price)
            .map(format_decimal)
            .unwrap_or_else(|| "--".to_string());
        let connection = if connected && self.healthy {
            tr("Healthy")
        } else if connected {
            tr("Degraded")
        } else {
            tr("Disconnected")
        };
        let latency = if self.is_simulation() {
            selected_chart
                .as_ref()
                .and_then(|source| source.market_latency_ms)
                .map(|latency| format!("Market {latency} ms"))
                .unwrap_or_else(|| "Market -- ms".to_string())
        } else {
            self.latency_ms
                .map(|value| format!("API {value} ms"))
                .unwrap_or_else(|| "API -- ms".to_string())
        };
        let panel_header = row![
            column![
                text(format!("{}  {}", self.config.symbol, market_price))
                    .size(crate::style::text_size::SECTION),
                text(format!("{connection} | {latency}")).size(crate::style::text_size::SMALL),
            ]
            .width(Length::Fill),
        ]
        .align_y(Alignment::Center);

        let [open_action, close_action] = trade_actions(self.position_side);

        let order_ticket = column![
            self.chart_row(),
            row![
                pick_list(
                    PositionSide::TRADE,
                    Some(self.position_side),
                    Message::PositionSideChanged
                )
                .width(Length::Fill),
                pick_list(
                    OrderType::ALL,
                    Some(self.order_type),
                    Message::OrderTypeChanged
                )
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                pick_list(
                    MarginMode::ALL,
                    Some(self.margin_mode),
                    Message::MarginModeChanged
                )
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                container(labeled_input(
                    tr("Quantity"),
                    "0.000",
                    &self.quantity,
                    Message::QuantityChanged,
                ))
                .width(Length::Fill),
                container(
                    column![
                        text(tr("Limit price")).size(crate::style::text_size::SMALL),
                        row![
                            text_input(tr("Market"), &self.price)
                                .on_input(Message::PriceChanged)
                                .width(Length::Fill),
                            icon_button(
                                crate::style::LucideIcon::MousePointer2,
                                tr("Use best bid/ask"),
                                Message::FillBestPrice,
                            ),
                        ]
                        .spacing(2)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(2),
                )
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                container(labeled_input(
                    tr("Risk (%)"),
                    "1.0",
                    &self.risk_percent,
                    Message::RiskPercentChanged,
                ))
                .width(Length::Fill),
                container(labeled_input(
                    tr("Reward ratio"),
                    "2.0",
                    &self.reward_ratio,
                    Message::RewardRatioChanged,
                ))
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                container(labeled_input(
                    tr("Stop loss"),
                    tr("Price"),
                    &self.stop_loss,
                    Message::StopLossChanged,
                ))
                .width(Length::Fill),
                container(labeled_input(
                    tr("Take profit"),
                    tr("Price"),
                    &self.take_profit,
                    Message::TakeProfitChanged,
                ))
                .width(Length::Fill),
            ]
            .spacing(6),
            labeled_input(
                tr("Leverage"),
                "1-125",
                &self.leverage,
                Message::LeverageChanged,
            ),
            row![
                text(format!(
                    "Balance allocation {}%",
                    self.config.allocation_percent
                ))
                .width(Length::Fill),
                slider(
                    0..=100,
                    self.config.allocation_percent,
                    Message::AllocationChanged
                )
                .width(Length::FillPortion(2)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(self.risk_preview())
                .size(crate::style::text_size::SMALL)
                .width(Length::Fill),
            row![
                button(
                    text(if self.busy {
                        tr("Working")
                    } else {
                        open_action.2
                    })
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                )
                .on_press_maybe(
                    (!self.busy).then_some(Message::PlaceOrder(open_action.0, open_action.1))
                )
                .style(if open_action.0 == Side::Buy {
                    crate::style::button::trade_buy
                } else {
                    crate::style::button::trade_sell
                })
                .width(Length::Fill),
                button(
                    text(if self.busy {
                        tr("Working")
                    } else {
                        close_action.2
                    })
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                )
                .on_press_maybe(
                    (!self.busy).then_some(Message::PlaceOrder(close_action.0, close_action.1))
                )
                .style(if close_action.0 == Side::Buy {
                    crate::style::button::trade_buy
                } else {
                    crate::style::button::trade_sell
                })
                .width(Length::Fill),
            ]
            .spacing(6),
        ]
        .spacing(6);

        let positions = self.positions_view();
        let orders = self.orders_view();
        let history = self.history_view(timezone);
        // Event contracts are simulation only; the tab bar exists only there.
        let event_tab = self.is_simulation() && self.config.event_tab;
        let tabs: Element<'_, Message> = if self.is_simulation() {
            let tab = |label: &'static str, active: bool, event_tab: bool| {
                button(text(label).align_x(Alignment::Center).width(Length::Fill))
                    .on_press(Message::EventTabChanged(event_tab))
                    .style(move |theme, status| {
                        crate::style::button::transparent(theme, status, active)
                    })
                    .width(Length::Fill)
            };
            row![
                tab(tr("Futures"), !event_tab, false),
                tab(tr("Event contracts"), event_tab, true),
            ]
            .spacing(6)
            .into()
        } else {
            column![].into()
        };
        // Chart edits confirm in a popup on the chart, not here.
        let confirmation: Element<'_, Message> = if let Some(action) = self
            .pending_action
            .as_ref()
            .filter(|_| self.chart_confirm.is_none())
        {
            let summary = match action {
                PendingAction::Place { request: order, .. } => format!(
                    "Confirm {} {} {} at {}",
                    order.side,
                    order.quantity,
                    order.symbol,
                    order
                        .price
                        .map(format_decimal)
                        .unwrap_or_else(|| "market".to_string())
                ),
                PendingAction::Cancel { order_id, .. } => {
                    format!("Confirm cancel order {order_id}")
                }
                PendingAction::CancelAll { symbol } => {
                    format!("Confirm cancel all open {symbol} orders")
                }
                PendingAction::Close(position) => format!(
                    "Confirm market close {} {}",
                    position.symbol,
                    position.quantity.abs()
                ),
                PendingAction::Protect {
                    position,
                    stop_loss,
                    take_profit,
                    existing: _,
                } => format!(
                    "Confirm protection for {} | SL {} | TP {}",
                    position.symbol,
                    stop_loss
                        .map(format_price_2)
                        .unwrap_or_else(|| "--".to_string()),
                    take_profit
                        .map(format_price_2)
                        .unwrap_or_else(|| "--".to_string())
                ),
                PendingAction::ModifyLimit { order, price } => format!(
                    "Confirm move order {} to {}",
                    order.order_id,
                    format_decimal(*price)
                ),
            };
            container(
                row![
                    text(summary).width(Length::Fill),
                    button(text(tr("Cancel"))).on_press(Message::CancelAction),
                    button(text(if self.config.testnet {
                        tr("Confirm")
                    } else {
                        tr("Confirm live order")
                    }))
                    .on_press(Message::ConfirmAction),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(10)
            .style(crate::style::chart_modal)
            .into()
        } else {
            column![].into()
        };

        let base = container(crate::style::thin_scrollable(
            if event_tab {
                column![panel_header, tabs, self.event_contracts_view(timezone),]
            } else {
                column![
                    panel_header,
                    tabs,
                    account_header,
                    order_ticket,
                    confirmation,
                    text(&self.status).size(crate::style::text_size::SMALL),
                    rule::horizontal(1),
                    positions,
                    rule::horizontal(1),
                    orders,
                    rule::horizontal(1),
                    history,
                ]
            }
            .spacing(10)
            .padding(padding::all(12)),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

        if let Some(editor) = &self.protection_editor {
            let position = self.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .positions
                    .iter()
                    .find(|position| position.symbol == editor.symbol)
            });
            let fee = self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.commission.taker)
                .unwrap_or_default();
            let preview = [
                optional_price(&editor.stop_loss).and_then(|price| {
                    position.map(|position| {
                        format!("SL PnL {:.2} USDT", position.estimated_pnl_at(price, fee))
                    })
                }),
                optional_price(&editor.take_profit).and_then(|price| {
                    position.map(|position| {
                        format!("TP PnL {:.2} USDT", position.estimated_pnl_at(price, fee))
                    })
                }),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" | ");
            let modal = container(
                column![
                    row![
                        text(format!("TP/SL - {}", editor.symbol))
                            .size(crate::style::text_size::TITLE)
                            .width(Length::Fill),
                        icon_button(
                            crate::style::LucideIcon::X,
                            tr("Close TP/SL editor"),
                            Message::CloseProtectionEditor,
                        ),
                    ]
                    .align_y(Alignment::Center),
                    labeled_input(
                        tr("Stop loss"),
                        tr("Price"),
                        &editor.stop_loss,
                        Message::EditStopLossChanged,
                    ),
                    labeled_input(
                        tr("Take profit"),
                        tr("Price"),
                        &editor.take_profit,
                        Message::EditTakeProfitChanged,
                    ),
                    text(preview).size(crate::style::text_size::SMALL),
                    button(
                        text(tr("Update TP/SL"))
                            .align_x(Alignment::Center)
                            .width(Length::Fill)
                    )
                    .on_press(Message::SaveProtection)
                    .width(Length::Fill),
                ]
                .spacing(10),
            )
            .width(360)
            .padding(16)
            .style(crate::style::chart_modal);
            crate::modal::pane::stack_modal(
                base,
                modal,
                Message::CloseProtectionEditor,
                iced::Padding::new(24.0),
                Alignment::Center,
            )
        } else if self.show_account_analytics {
            let analytics = account_analytics(
                self.snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.trade_history.as_slice())
                    .unwrap_or_default(),
            );
            let profit_factor = if analytics.gross_loss > f64::EPSILON {
                format!("{:.2}", analytics.profit_factor())
            } else {
                "--".to_string()
            };
            let modal = container(
                column![
                    row![
                        text(tr("Account performance"))
                            .size(crate::style::text_size::TITLE)
                            .width(Length::Fill),
                        icon_button(
                            crate::style::LucideIcon::X,
                            tr("Close account performance"),
                            Message::ToggleAccountAnalytics,
                        ),
                    ]
                    .align_y(Alignment::Center),
                    rule::horizontal(1),
                    row![
                        text(tr("Net realized PnL")).width(Length::Fill),
                        text(format!("{:.2} USDT", analytics.net_pnl)),
                    ],
                    row![
                        text(tr("Win rate (closed fills)")).width(Length::Fill),
                        text(format!("{:.1}%", analytics.win_rate())),
                    ],
                    row![
                        text(tr("Closed fills")).width(Length::Fill),
                        text(format!(
                            "{} ({}W / {}L)",
                            analytics.closed_fills, analytics.wins, analytics.losses
                        )),
                    ],
                    row![
                        text(tr("Profit factor")).width(Length::Fill),
                        text(profit_factor),
                    ],
                    row![
                        text(tr("Gross profit / loss")).width(Length::Fill),
                        text(format!(
                            "{:.2} / -{:.2}",
                            analytics.gross_profit, analytics.gross_loss
                        )),
                    ],
                    row![
                        text(tr("Average win / loss")).width(Length::Fill),
                        text(format!(
                            "{:.2} / {:.2}",
                            analytics.average_win, analytics.average_loss
                        )),
                    ],
                    row![
                        text(tr("Best / worst fill")).width(Length::Fill),
                        text(format!(
                            "{:.2} / {:.2}",
                            analytics.best_trade, analytics.worst_trade
                        )),
                    ],
                    text(tr(
                        "USDT fees are included; win rate is based on fills with realized PnL."
                    ))
                    .size(crate::style::text_size::SMALL),
                ]
                .spacing(10),
            )
            .width(420)
            .padding(16)
            .style(crate::style::chart_modal);
            crate::modal::pane::stack_modal(
                base,
                modal,
                Message::ToggleAccountAnalytics,
                iced::Padding::new(24.0),
                Alignment::Center,
            )
        } else {
            base
        }
    }

    fn positions_view(&self) -> Element<'_, Message> {
        iced::widget::responsive(move |bounds| {
            let compact = bounds.width < 900.0;
            let narrow = bounds.width < 520.0;
            let mut header = row![text(tr("Symbol")).width(Length::FillPortion(2))];
            if !narrow {
                header = header.push(text(tr("Side")).width(Length::FillPortion(1)));
            }
            header = header.push(text(tr("Qty")).width(Length::FillPortion(2)));
            if !compact {
                header = header
                    .push(text(tr("Mark")).width(Length::FillPortion(2)))
                    .push(text(tr("Entry / BE")).width(Length::FillPortion(3)))
                    .push(text(tr("Liq")).width(Length::FillPortion(2)));
            } else if !narrow {
                header = header.push(text(tr("Entry")).width(Length::FillPortion(2)));
            }
            header = header.push(text(tr("PnL USDT")).width(Length::FillPortion(2)));
            if !compact {
                header = header.push(text(tr("Lev")).width(Length::FillPortion(1)));
            }
            header = header.push(text(tr("Actions")).width(Length::Fixed(104.0)));

            let mut list = column![
                row![
                    text(tr("Positions"))
                        .size(crate::style::text_size::SECTION)
                        .width(Length::Fill),
                    icon_button(
                        crate::style::LucideIcon::ChartNoAxesColumnIncreasing,
                        tr("Account performance"),
                        Message::ToggleAccountAnalytics,
                    ),
                ]
                .align_y(Alignment::Center),
                header.spacing(6),
                rule::horizontal(1),
            ]
            .spacing(5);
            if let Some(snapshot) = &self.snapshot {
                for (index, position) in snapshot.positions.iter().enumerate() {
                    let positive = position.unrealized_pnl >= 0.0;
                    let mut fields = row![text(&position.symbol).width(Length::FillPortion(2))];
                    if !narrow {
                        fields = fields.push(
                            text(if position.is_long() { "LONG" } else { "SHORT" })
                                .width(Length::FillPortion(1)),
                        );
                    }
                    fields = fields.push(
                        text(format_decimal(position.quantity.abs())).width(Length::FillPortion(2)),
                    );
                    if !compact {
                        fields = fields
                            .push(
                                text(format_decimal(position.mark_price))
                                    .width(Length::FillPortion(2)),
                            )
                            .push(
                                text(format!(
                                    "{} / {}",
                                    format_decimal(position.entry_price),
                                    format_decimal(position.economic_break_even_price)
                                ))
                                .width(Length::FillPortion(3)),
                            )
                            .push(
                                text(if position.liquidation_price > 0.0 {
                                    format_decimal(position.liquidation_price)
                                } else {
                                    "--".to_string()
                                })
                                .width(Length::FillPortion(2)),
                            );
                    } else if !narrow {
                        fields = fields.push(
                            text(format_decimal(position.entry_price))
                                .width(Length::FillPortion(2)),
                        );
                    }
                    fields = fields.push(
                        text(format!("{:.2}", position.unrealized_pnl))
                            .width(Length::FillPortion(2))
                            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                                color: Some(if positive {
                                    theme.extended_palette().success.base.color
                                } else {
                                    theme.extended_palette().danger.base.color
                                }),
                            }),
                    );
                    if !compact {
                        fields = fields.push(
                            text(format!("{}x", position.leverage)).width(Length::FillPortion(1)),
                        );
                    }
                    fields = fields.push(
                        row![
                            icon_button(
                                crate::style::LucideIcon::TrendingUp,
                                tr("Edit TP/SL"),
                                Message::EditProtection(index),
                            ),
                            icon_button(
                                crate::style::LucideIcon::X,
                                tr("Close position"),
                                Message::ClosePosition(index),
                            ),
                        ]
                        .spacing(4)
                        .width(Length::Fixed(104.0)),
                    );
                    list = list.push(fields.spacing(6).align_y(Alignment::Center));
                }
            }
            if self
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.positions.is_empty())
            {
                list = list.push(text(tr("No open positions")));
            }
            list.into()
        })
        .into()
    }

    fn orders_view(&self) -> Element<'_, Message> {
        iced::widget::responsive(move |bounds| {
            let compact = bounds.width < 700.0;
            let narrow = bounds.width < 430.0;
            let mut header = row![text(tr("Symbol")).width(Length::FillPortion(2))];
            if !narrow {
                header = header
                    .push(text(tr("Side")).width(Length::FillPortion(1)))
                    .push(text(tr("Type")).width(Length::FillPortion(2)));
            }
            if !compact {
                header = header.push(text(tr("Qty")).width(Length::FillPortion(2)));
            }
            header = header.push(text(tr("Price / Trigger")).width(Length::FillPortion(3)));
            if !compact {
                header = header.push(text(tr("Flags")).width(Length::FillPortion(2)));
            }
            header = header.push(text(tr("Action")).width(Length::Fixed(64.0)));

            let mut list = column![
                row![
                    text(tr("Open orders"))
                        .size(crate::style::text_size::SECTION)
                        .width(Length::Fill),
                    icon_button(
                        crate::style::LucideIcon::Trash2,
                        tr("Cancel all orders"),
                        Message::CancelAllOrders,
                    ),
                ]
                .align_y(Alignment::Center),
                header.spacing(6),
                rule::horizontal(1),
            ]
            .spacing(5);
            if let Some(snapshot) = &self.snapshot {
                for order in &snapshot.open_orders {
                    let mut fields = row![text(&order.symbol).width(Length::FillPortion(2))];
                    if !narrow {
                        fields = fields
                            .push(text(order.side.api()).width(Length::FillPortion(1)))
                            .push(text(&order.order_type).width(Length::FillPortion(2)));
                    }
                    if !compact {
                        fields = fields.push(
                            text(format_decimal(order.quantity)).width(Length::FillPortion(2)),
                        );
                    }
                    fields = fields.push(
                        text(format!(
                            "{} / {}",
                            if order.price > 0.0 {
                                format_decimal(order.price)
                            } else {
                                "--".to_string()
                            },
                            if order.stop_price > 0.0 {
                                format_decimal(order.stop_price)
                            } else {
                                "--".to_string()
                            }
                        ))
                        .width(Length::FillPortion(3)),
                    );
                    if !compact {
                        fields = fields.push(
                            text(if order.reduce_only {
                                tr("Reduce")
                            } else {
                                tr("Open")
                            })
                            .width(Length::FillPortion(2)),
                        );
                    }
                    fields = fields.push(
                        container(icon_button(
                            crate::style::LucideIcon::X,
                            tr("Cancel order"),
                            Message::CancelOrder(
                                order.symbol.clone(),
                                order.order_id,
                                order.is_algo,
                            ),
                        ))
                        .width(Length::Fixed(64.0)),
                    );
                    list = list.push(fields.spacing(6).align_y(Alignment::Center));
                }
            }
            if self
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.open_orders.is_empty())
            {
                list = list.push(text(tr("No open orders")));
            }
            list.into()
        })
        .into()
    }

    fn history_view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        iced::widget::responsive(move |bounds| {
            let compact = bounds.width < 700.0;
            let narrow = bounds.width < 460.0;
            let trades = self
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .trade_history
                        .iter()
                        .filter(|trade| trade.symbol == self.config.symbol)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let is_empty = trades.is_empty();
            let mut header = row![text(tr("Time")).width(Length::FillPortion(3))];
            header = header.push(text(tr("Side")).width(Length::FillPortion(1)));
            if !narrow {
                header = header.push(text(tr("Qty")).width(Length::FillPortion(2)));
            }
            header = header.push(text(tr("Price")).width(Length::FillPortion(2)));
            header = header.push(text(tr("PnL USDT")).width(Length::FillPortion(2)));
            header = header.push(text(tr("Fee")).width(Length::FillPortion(2)));
            let mut list = column![
                text(tr("Trade history")).size(crate::style::text_size::SECTION),
                header.spacing(6),
                rule::horizontal(1),
            ]
            .spacing(5);
            for trade in trades.into_iter().rev().take(100) {
                // Realized PnL before fees; the fee has its own column.
                let pnl = trade.realized_pnl;
                let pnl_positive = pnl >= 0.0;
                let time = timezone
                    .format_with_kind(
                        trade.time_ms.min(i64::MAX as u64) as i64,
                        data::config::timezone::TimeLabelKind::Custom("%m-%d %H:%M:%S"),
                    )
                    .unwrap_or_else(|| "--".to_string());
                let mut fields = row![text(time).width(Length::FillPortion(3))];
                fields = fields.push(text(trade.side.api()).width(Length::FillPortion(1)));
                if !narrow {
                    fields = fields
                        .push(text(format_decimal(trade.quantity)).width(Length::FillPortion(2)));
                }
                fields = fields
                    .push(text(format_decimal(trade.price)).width(Length::FillPortion(2)))
                    .push(
                        text(if pnl.abs() < f64::EPSILON {
                            "--".to_string()
                        } else {
                            format!("{pnl:+.2}")
                        })
                        .width(Length::FillPortion(2))
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(if pnl_positive {
                                theme.extended_palette().success.base.color
                            } else {
                                theme.extended_palette().danger.base.color
                            }),
                        }),
                    );
                fields = fields.push(
                    text(if compact || trade.commission_asset == "USDT" {
                        format!("-{:.2}", trade.commission)
                    } else {
                        format!(
                            "-{} {}",
                            format_decimal(trade.commission),
                            trade.commission_asset
                        )
                    })
                    .width(Length::FillPortion(2))
                    .style(|theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(theme.extended_palette().background.strong.text),
                    }),
                );
                list = list.push(fields.spacing(6).align_y(Alignment::Center));
            }
            if is_empty {
                list = list.push(text(tr("No trade history for this symbol")));
            }
            list.into()
        })
        .into()
    }

    fn event_contracts_view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        let now = local_time_ms();
        let price = self
            .selected_source()
            .and_then(|source| source.latest_price);
        let contracts = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.event_contracts.as_slice())
            .unwrap_or_default();
        let signed = |content: String, good: bool| {
            text(content).style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(if good {
                    theme.extended_palette().success.base.color
                } else {
                    theme.extended_palette().danger.base.color
                }),
            })
        };
        let time = |ms: u64| {
            timezone
                .format_with_kind(
                    ms.min(i64::MAX as u64) as i64,
                    data::config::timezone::TimeLabelKind::Custom("%m-%d %H:%M:%S"),
                )
                .unwrap_or_else(|| "--".to_string())
        };
        let open_count = contracts.iter().filter(|c| c.is_open()).count();
        let stats = event::stats(contracts);
        let usdt = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.balances.iter().find(|b| b.asset == "USDT"));
        let stat = |label: &'static str, value: Element<'static, Message>| {
            row![text(label).width(Length::Fill), value].spacing(8)
        };
        let header = column![
            stat(
                "USDT / Available",
                text(usdt.map_or_else(
                    || "--".to_string(),
                    |b| format!("{:.2} / {:.2}", b.wallet, b.available)
                ))
                .into()
            ),
            stat(
                tr("Net PnL"),
                signed(format!("{:+.2} USDT", stats.net), stats.net >= 0.0).into()
            ),
            stat(
                tr("Win rate"),
                text(format!(
                    "{:.1}% ({}W / {}L)",
                    stats.win_rate(),
                    stats.wins,
                    stats.settled - stats.wins
                ))
                .into()
            ),
            stat(
                tr("Total profit / loss"),
                row![
                    signed(format!("+{:.2}", stats.gross_profit), true),
                    text("/"),
                    signed(format!("-{:.2}", stats.gross_loss), false),
                ]
                .spacing(4)
                .into()
            ),
            stat(
                tr("Open / at stake"),
                text(format!(
                    "{}/{} | {:.2} USDT",
                    stats.open,
                    event::MAX_OPEN,
                    stats.open_stake
                ))
                .into()
            ),
        ]
        .spacing(3);

        let mut panel = column![
            header,
            self.chart_row(),
            text(&self.status).size(crate::style::text_size::SMALL),
            rule::horizontal(1),
            row![
                text(tr("Event contracts"))
                    .size(crate::style::text_size::SECTION)
                    .width(Length::Fill),
                text(format!("{open_count}/{} open", event::MAX_OPEN))
                    .size(crate::style::text_size::SMALL),
            ]
            .align_y(Alignment::Center),
            row![
                pick_list(
                    EventDuration::ALL,
                    Some(self.event_duration),
                    Message::EventDurationChanged
                ),
                text(format!(
                    "Win +{:.0}%",
                    self.event_duration.payout_rate() * 100.0
                )),
                text_input("USDT", &self.event_stake)
                    .on_input(Message::EventStakeChanged)
                    .width(Length::Fill),
                text(format!("{}-{} USDT", event::MIN_STAKE, event::MAX_STAKE))
                    .size(crate::style::text_size::SMALL),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                button(
                    text(tr("Up"))
                        .align_x(Alignment::Center)
                        .width(Length::Fill)
                )
                .on_press(Message::OpenEventContract(true))
                .style(crate::style::button::trade_buy)
                .width(Length::Fill),
                button(
                    text(tr("Down"))
                        .align_x(Alignment::Center)
                        .width(Length::Fill)
                )
                .on_press(Message::OpenEventContract(false))
                .style(crate::style::button::trade_sell)
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                checkbox(self.config.event_show_open)
                    .label(tr("Chart: open"))
                    .on_toggle(Message::EventShowOpenChanged),
                checkbox(self.config.event_show_history)
                    .label(tr("Chart: history"))
                    .on_toggle(Message::EventShowHistoryChanged),
            ]
            .spacing(12),
        ]
        .spacing(6);

        if open_count > 0 {
            panel = panel.push(
                row![
                    text(tr("Side")).width(Length::FillPortion(2)),
                    text(tr("Stake")).width(Length::FillPortion(3)),
                    text(tr("Open")).width(Length::FillPortion(3)),
                    text(tr("Now")).width(Length::FillPortion(3)),
                    text(tr("Diff")).width(Length::FillPortion(3)),
                    text(tr("Left")).width(Length::FillPortion(2)),
                    text(tr("Result")).width(Length::FillPortion(2)),
                ]
                .spacing(6),
            );
        }
        for contract in contracts.iter().filter(|c| c.is_open()) {
            let left = contract.expiry_ms.saturating_sub(now) / 1_000;
            // Each contract follows its own symbol's chart, not just the selected one.
            let price = self
                .chart_sources
                .iter()
                .find(|source| source.symbol == contract.symbol)
                .and_then(|source| source.latest_price)
                .or(price.filter(|_| contract.symbol == self.config.symbol));
            let winning = price.is_some_and(|p| contract.wins_at(p));
            let diff = price.map(|p| p - contract.open_price);
            panel = panel.push(
                row![
                    signed(
                        format!(
                            "{} {}",
                            if contract.up { "UP" } else { "DOWN" },
                            contract.duration
                        ),
                        contract.up
                    )
                    .width(Length::FillPortion(2)),
                    text(format!("{} {:.0}u", contract.symbol, contract.stake))
                        .width(Length::FillPortion(3)),
                    text(format_decimal(contract.open_price)).width(Length::FillPortion(3)),
                    text(price.map_or_else(|| "--".to_string(), format_decimal))
                        .width(Length::FillPortion(3)),
                    signed(
                        diff.map_or_else(
                            || "--".to_string(),
                            |d| format!("{d:+.2} ({:+.3}%)", d / contract.open_price * 100.0)
                        ),
                        winning
                    )
                    .width(Length::FillPortion(3)),
                    text(format!("{:02}:{:02}", left / 60, left % 60))
                        .width(Length::FillPortion(2)),
                    signed(
                        if winning {
                            format!("+{:.2}", contract.stake * contract.payout_rate)
                        } else {
                            format!("-{:.2}", contract.stake)
                        },
                        winning
                    )
                    .width(Length::FillPortion(2)),
                ]
                .spacing(6),
            );
        }

        let settled = contracts
            .iter()
            .filter(|c| !c.is_open())
            .collect::<Vec<_>>();
        if !settled.is_empty() {
            panel = panel.push(rule::horizontal(1));
            panel = panel.push(text(tr("History")).size(crate::style::text_size::SECTION));
            for contract in settled.into_iter().rev() {
                let pnl = contract.pnl().unwrap_or_default();
                panel = panel.push(
                    row![
                        text(time(contract.open_time_ms)).width(Length::FillPortion(3)),
                        signed(
                            format!(
                                "{} {}",
                                if contract.up { "UP" } else { "DOWN" },
                                contract.duration
                            ),
                            contract.up
                        )
                        .width(Length::FillPortion(2)),
                        text(format!(
                            "{} -> {}",
                            format_decimal(contract.open_price),
                            contract
                                .settle_price
                                .map_or_else(|| "--".to_string(), format_decimal)
                        ))
                        .width(Length::FillPortion(4)),
                        signed(format!("{pnl:+.2}"), pnl > 0.0).width(Length::FillPortion(2)),
                    ]
                    .spacing(6),
                );
            }
        }
        panel.into()
    }

    fn calculate_risk_levels(&mut self) {
        let risk = self
            .risk_percent
            .parse::<f64>()
            .ok()
            .filter(|value| *value > 0.0 && *value < 100.0);
        let reward = self
            .reward_ratio
            .parse::<f64>()
            .ok()
            .filter(|value| *value > 0.0);
        let selected_position = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .positions
                .iter()
                .find(|position| position.symbol == self.config.symbol)
        });
        let market_reference = selected_position
            .map(|position| position.mark_price)
            .or_else(|| optional_price(&self.price))
            .or_else(|| {
                self.selected_chart.and_then(|id| {
                    self.chart_sources
                        .iter()
                        .find(|source| source.pane_id == id)
                        .and_then(|source| source.latest_price)
                })
            });
        let (Some(risk), Some(reward), Some(reference)) = (risk, reward, market_reference) else {
            return;
        };
        let commission = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.commission)
            .unwrap_or_default();
        let distance = reference * risk / 100.0;
        let is_long = selected_position
            .map(Position::is_long)
            .unwrap_or(self.side == Side::Buy);
        let break_even = selected_position.map_or_else(
            || {
                let entry_fee = if self.order_type == OrderType::Limit {
                    commission.maker
                } else {
                    commission.taker
                };
                let direction = if is_long { 1.0 } else { -1.0 };
                reference + direction * reference * entry_fee
            },
            |position| position.economic_break_even_price,
        );
        let exit_fee = commission.taker.max(0.0);
        let (stop_loss, take_profit) =
            net_risk_levels(break_even, distance, reward, exit_fee, is_long);
        self.stop_loss = format_price_2(stop_loss);
        self.take_profit = format_price_2(take_profit);
    }

    fn risk_preview(&self) -> String {
        let Some(snapshot) = &self.snapshot else {
            return String::new();
        };
        let Some(position) = snapshot
            .positions
            .iter()
            .find(|position| position.symbol == self.config.symbol)
        else {
            return String::new();
        };
        let fee = snapshot.commission.taker;
        let sl = optional_price(&self.stop_loss)
            .map(|price| format!("SL PnL {:.2} USDT", position.estimated_pnl_at(price, fee)));
        let tp = optional_price(&self.take_profit)
            .map(|price| format!("TP PnL {:.2} USDT", position.estimated_pnl_at(price, fee)));
        [sl, tp]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

fn numeric_input(value: String) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '.')
        .collect()
}

/// A fee rate in percent: 0 up to 1 %.
fn fee_input(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|pct| pct.is_finite() && (0.0..=1.0).contains(pct))
}

fn integer_input(value: String) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect()
}

fn simulated_snapshot(balance: u64) -> AccountSnapshot {
    AccountSnapshot {
        balances: vec![Balance {
            asset: "USDT".to_string(),
            wallet: balance as f64,
            available: balance as f64,
            unrealized_pnl: 0.0,
        }],
        commission: CommissionRate {
            maker: 0.0002,
            taker: 0.0004,
        },
        liquidation_fee_rate: DEFAULT_LIQUIDATION_FEE_RATE,
        hedge_mode: false,
        updated_at_ms: local_time_ms(),
        ..Default::default()
    }
}

fn simulation_path(account_id: &str) -> String {
    format!("trading/simulation/{account_id}.json")
}

fn load_simulated_snapshot(account_id: &str, balance: u64) -> AccountSnapshot {
    // Tests must never read or write the real simulation account.
    if cfg!(test) {
        return simulated_snapshot(balance);
    }
    std::fs::read_to_string(data::data_path(Some(&simulation_path(account_id))))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .or_else(|| merge_legacy_simulations(account_id))
        .unwrap_or_else(|| simulated_snapshot(balance))
}

/// Older versions kept one simulation file per trading panel. The first time
/// the shared account is opened, merge every one of them into it; the old files
/// stay on disk untouched.
fn merge_legacy_simulations(account_id: &str) -> Option<AccountSnapshot> {
    if account_id != data::config::trading::SHARED_ACCOUNT_ID {
        return None;
    }
    let dir = data::data_path(Some("trading/simulation"));
    let mut accounts = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let json = std::fs::read_to_string(entry.path()).ok()?;
            Some((
                modified,
                serde_json::from_str::<AccountSnapshot>(&json).ok()?,
            ))
        })
        .collect::<Vec<_>>();
    accounts.sort_by_key(|(modified, _)| *modified);
    let merged = merge_snapshots(accounts.into_iter().map(|(_, snapshot)| snapshot).collect())?;
    if let Err(error) = save_simulated_snapshot(account_id, &merged) {
        log::error!("Failed to save merged simulation account: {error}");
    }
    Some(merged)
}

/// Oldest first. Balance, positions and orders come from the newest account;
/// trade history and event contracts from all of them.
fn merge_snapshots(snapshots: Vec<AccountSnapshot>) -> Option<AccountSnapshot> {
    let mut trades = Vec::new();
    let mut events = Vec::new();
    let mut newest = None;
    for snapshot in snapshots {
        trades.extend(snapshot.trade_history.iter().cloned());
        events.extend(snapshot.event_contracts.iter().cloned());
        newest = Some(snapshot);
    }
    let mut merged = newest?;
    trades.sort_by_key(|trade: &TradeHistory| (trade.time_ms, trade.trade_id));
    trades.dedup_by_key(|trade| (trade.time_ms, trade.trade_id));
    events.sort_by_key(|event: &EventContract| (event.open_time_ms, event.id));
    events.dedup_by_key(|event| (event.open_time_ms, event.id));
    merged.trade_history = trades;
    merged.event_contracts = events;
    Some(merged)
}

#[cfg(test)]
mod shared_account_tests {
    use super::*;

    #[test]
    fn merge_keeps_every_history_and_newest_balance() {
        let trade = |id, time_ms| TradeHistory {
            trade_id: id,
            order_id: id,
            symbol: "BTCUSDT".into(),
            side: Side::Buy,
            quantity: 1.0,
            price: 1.0,
            realized_pnl: 0.0,
            commission: 0.0,
            commission_asset: "USDT".into(),
            time_ms,
        };
        let old = AccountSnapshot {
            trade_history: vec![trade(1, 10), trade(2, 20)],
            ..simulated_snapshot(1_000)
        };
        let new = AccountSnapshot {
            trade_history: vec![trade(2, 20), trade(3, 5)],
            ..simulated_snapshot(5_000)
        };
        let merged = merge_snapshots(vec![old, new.clone()]).unwrap();
        let ids: Vec<_> = merged.trade_history.iter().map(|t| t.trade_id).collect();
        assert_eq!(ids, [3, 1, 2]);
        assert_eq!(merged.balances.len(), new.balances.len());
        assert_eq!(
            merged.balances.first().map(|b| b.wallet),
            new.balances.first().map(|b| b.wallet)
        );
        assert!(merge_snapshots(Vec::new()).is_none());
    }
}

fn save_simulated_snapshot(account_id: &str, snapshot: &AccountSnapshot) -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    let json = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    data::write_json_to_file(&json, &simulation_path(account_id)).map_err(|error| error.to_string())
}

fn simulation_order_id(snapshot: &AccountSnapshot) -> u64 {
    local_time_ms().saturating_mul(100) + snapshot.open_orders.len() as u64
}

/// A taker fill (market orders, stop / take-profit triggers).
#[allow(clippy::too_many_arguments)]
fn simulated_fill(
    snapshot: &mut AccountSnapshot,
    symbol: &str,
    side: Side,
    quantity: f64,
    price: f64,
    leverage: u16,
    margin_mode: MarginMode,
    reduce_only: bool,
) -> Result<(), String> {
    let taker = snapshot.commission.taker;
    simulated_fill_with_fee(
        snapshot,
        symbol,
        side,
        quantity,
        price,
        leverage,
        margin_mode,
        reduce_only,
        taker,
    )
}

#[allow(clippy::too_many_arguments)]
fn simulated_fill_with_fee(
    snapshot: &mut AccountSnapshot,
    symbol: &str,
    side: Side,
    quantity: f64,
    price: f64,
    leverage: u16,
    margin_mode: MarginMode,
    reduce_only: bool,
    fee_rate: f64,
) -> Result<(), String> {
    if !quantity.is_finite() || quantity <= 0.0 || !price.is_finite() || price <= 0.0 {
        return Err("Simulation fill has an invalid quantity or price.".to_string());
    }
    let signed = if side == Side::Buy {
        quantity
    } else {
        -quantity
    };
    let position_index = snapshot
        .positions
        .iter()
        .position(|position| position.symbol == symbol);
    if reduce_only
        && position_index
            .is_none_or(|index| snapshot.positions[index].quantity.signum() == signed.signum())
    {
        return Err("Reduce-only order would increase the simulated position.".to_string());
    }
    let opening_quantity = position_index.map_or(quantity, |index| {
        let current = snapshot.positions[index].quantity;
        if current.signum() == signed.signum() {
            quantity
        } else {
            (quantity - current.abs()).max(0.0)
        }
    });
    let required =
        opening_quantity * price / f64::from(leverage.max(1)) + quantity * price * fee_rate;
    let available = snapshot
        .balances
        .iter()
        .find(|balance| balance.asset == "USDT")
        .map_or(0.0, |balance| balance.available);
    if required > available && !reduce_only {
        return Err(tr("Insufficient simulated available balance.").to_string());
    }

    let commission = quantity * price * fee_rate;
    let mut realized_pnl = 0.0;
    let mut wallet_delta = -commission;
    match position_index {
        Some(index) => {
            let position = &mut snapshot.positions[index];
            let fill = if reduce_only {
                signed.signum() * quantity.min(position.quantity.abs())
            } else {
                signed
            };
            let old_quantity = position.quantity;
            let new_quantity = old_quantity + fill;
            // Break-even carries every fee and realized PnL since the position
            // opened, like Binance's: qty × (P − BE) is the net PnL at price P.
            let economic = if new_quantity.abs() <= f64::EPSILON {
                position.economic_break_even_price
            } else if new_quantity.signum() != old_quantity.signum() {
                let opening_fee = commission * (new_quantity.abs() / quantity);
                price + opening_fee / new_quantity
            } else {
                (position.economic_break_even_price * old_quantity + price * fill + commission)
                    / new_quantity
            };
            if old_quantity.signum() == fill.signum() {
                position.entry_price = (position.entry_price * old_quantity.abs()
                    + price * fill.abs())
                    / new_quantity.abs();
            } else {
                let closed = old_quantity.abs().min(fill.abs());
                realized_pnl = (price - position.entry_price) * closed * old_quantity.signum();
                wallet_delta += realized_pnl;
                if new_quantity.abs() > f64::EPSILON
                    && new_quantity.signum() != old_quantity.signum()
                {
                    position.entry_price = price;
                }
            }
            position.quantity = new_quantity;
            position.mark_price = price;
            position.break_even_price = economic;
            position.economic_break_even_price = economic;
            position.leverage = leverage.max(1);
            position.margin_mode = margin_mode;
        }
        None => snapshot.positions.push(Position {
            symbol: symbol.to_string(),
            side: PositionSide::Both,
            quantity: signed,
            entry_price: price,
            mark_price: price,
            break_even_price: price + commission / signed,
            economic_break_even_price: price + commission / signed,
            liquidation_price: 0.0,
            leverage: leverage.max(1),
            margin_mode,
            unrealized_pnl: 0.0,
            funding: 0.0,
            opened_at_ms: Some(local_time_ms()),
        }),
    }
    snapshot
        .positions
        .retain(|position| position.quantity.abs() > f64::EPSILON);
    let balance = snapshot
        .balances
        .iter_mut()
        .find(|balance| balance.asset == "USDT")
        .ok_or_else(|| "Simulation USDT balance is missing.".to_string())?;
    balance.wallet += wallet_delta;
    let time_ms = local_time_ms();
    let trade_id = time_ms.saturating_mul(100) + snapshot.trade_history.len() as u64;
    snapshot.trade_history.push(TradeHistory {
        trade_id,
        order_id: trade_id,
        symbol: symbol.to_string(),
        side,
        quantity,
        price,
        realized_pnl,
        commission,
        commission_asset: "USDT".to_string(),
        time_ms,
    });
    if snapshot.trade_history.len() > 1_000 {
        snapshot
            .trade_history
            .drain(..snapshot.trade_history.len() - 1_000);
    }
    refresh_simulated_account(snapshot, symbol, price);
    Ok(())
}

fn refresh_simulated_account(snapshot: &mut AccountSnapshot, symbol: &str, price: f64) {
    let liquidation_fee_rate = snapshot.liquidation_fee_rate;
    for position in &mut snapshot.positions {
        if position.symbol == symbol {
            position.mark_price = price;
            position.unrealized_pnl = position.quantity * (price - position.entry_price);
        }
    }
    let wallet = snapshot
        .balances
        .iter()
        .find(|balance| balance.asset == "USDT")
        .map_or(0.0, |balance| balance.wallet);
    let liquidation = (0..snapshot.positions.len())
        .map(|index| {
            let position = &snapshot.positions[index];
            if position.symbol != symbol {
                return position.liquidation_price;
            }
            match position.margin_mode {
                MarginMode::Isolated => estimated_liquidation_price(
                    position.entry_price,
                    position.leverage,
                    position.is_long(),
                    SIM_MAINTENANCE_MARGIN_RATE,
                    liquidation_fee_rate,
                ),
                MarginMode::Cross => cross_liquidation_price(
                    &snapshot.positions,
                    index,
                    wallet,
                    SIM_MAINTENANCE_MARGIN_RATE,
                    liquidation_fee_rate,
                ),
            }
            .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let mut unrealized = 0.0;
    let mut margin = 0.0;
    for (position, liquidation) in snapshot.positions.iter_mut().zip(liquidation) {
        position.liquidation_price = liquidation;
        unrealized += position.unrealized_pnl;
        margin +=
            position.quantity.abs() * position.mark_price / f64::from(position.leverage.max(1));
    }
    if let Some(balance) = snapshot
        .balances
        .iter_mut()
        .find(|balance| balance.asset == "USDT")
    {
        balance.unrealized_pnl = unrealized;
        balance.available = (balance.wallet + unrealized - margin).max(0.0);
    }
    snapshot.updated_at_ms = local_time_ms();
}

/// Maintenance reserve as a share of notional: maintenance margin plus the
/// liquidation fee, capped at half the initial margin. Binance gets the same
/// effect with lower maintenance rates on high-leverage brackets (125x: 0.8%
/// initial, 0.4% maintenance); uncapped, the price would land on the wrong
/// side of entry above ~57x.
fn maintenance_reserve_rate(leverage: u16, maintenance_margin_rate: f64, fee_rate: f64) -> f64 {
    (maintenance_margin_rate.max(0.0) + fee_rate.max(0.0)).min(0.5 / f64::from(leverage.max(1)))
}

/// Cross margin: the whole wallet backs the position. Liquidation is the
/// price where account equity (wallet, the other cross positions' PnL, minus
/// margin locked by isolated positions) falls to the maintenance reserve.
/// `None` when the wallet covers any move (a long never liquidates above 0).
fn cross_liquidation_price(
    positions: &[Position],
    index: usize,
    wallet: f64,
    maintenance_margin_rate: f64,
    liquidation_fee_rate: f64,
) -> Option<f64> {
    let position = positions.get(index)?;
    let quantity = position.quantity;
    if quantity.abs() <= f64::EPSILON || !position.entry_price.is_finite() {
        return None;
    }
    let mut equity = wallet;
    for (other_index, other) in positions.iter().enumerate() {
        if other_index == index {
            continue;
        }
        match other.margin_mode {
            MarginMode::Isolated => {
                equity -=
                    other.quantity.abs() * other.entry_price / f64::from(other.leverage.max(1));
            }
            MarginMode::Cross => {
                equity += other.unrealized_pnl
                    - other.quantity.abs()
                        * other.mark_price
                        * maintenance_reserve_rate(
                            other.leverage,
                            maintenance_margin_rate,
                            liquidation_fee_rate,
                        );
            }
        }
    }
    let rate = maintenance_reserve_rate(
        position.leverage,
        maintenance_margin_rate,
        liquidation_fee_rate,
    );
    // equity + q (P - entry) = |q| P rate, solved for P.
    let price =
        (quantity * position.entry_price - equity) / (quantity * (1.0 - quantity.signum() * rate));
    (price.is_finite() && price > 0.0).then_some(price)
}

fn estimated_liquidation_price(
    entry_price: f64,
    leverage: u16,
    is_long: bool,
    maintenance_margin_rate: f64,
    liquidation_fee_rate: f64,
) -> Option<f64> {
    if !entry_price.is_finite() || entry_price <= 0.0 || leverage == 0 {
        return None;
    }
    let margin_rate =
        maintenance_reserve_rate(leverage, maintenance_margin_rate, liquidation_fee_rate);
    let leverage = f64::from(leverage);
    let price = if is_long {
        entry_price * (1.0 - 1.0 / leverage) / (1.0 - margin_rate)
    } else {
        entry_price * (1.0 + 1.0 / leverage) / (1.0 + margin_rate)
    };
    price.is_finite().then_some(price.max(0.0))
}

fn add_simulated_protection(
    snapshot: &mut AccountSnapshot,
    symbol: &str,
    entry_side: Side,
    quantity: f64,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
) {
    snapshot.open_orders.retain(|order| {
        order.symbol != symbol
            || (!order.order_type.contains("STOP") && !order.order_type.contains("TAKE_PROFIT"))
    });
    let close_side = if entry_side == Side::Buy {
        Side::Sell
    } else {
        Side::Buy
    };
    for (order_type, stop_price) in [
        ("STOP_MARKET", stop_loss),
        ("TAKE_PROFIT_MARKET", take_profit),
    ] {
        if let Some(stop_price) = stop_price {
            let order_id = simulation_order_id(snapshot);
            snapshot.open_orders.push(OpenOrder {
                symbol: symbol.to_string(),
                order_id,
                side: close_side,
                order_type: order_type.to_string(),
                price: 0.0,
                stop_price,
                quantity,
                reduce_only: true,
                is_algo: true,
            });
        }
    }
}

fn match_simulated_orders(
    snapshot: &mut AccountSnapshot,
    symbol: &str,
    market: f64,
    fallback_leverage: u16,
    fallback_margin_mode: MarginMode,
) -> bool {
    let mut triggered = Vec::new();
    snapshot.open_orders.retain(|order| {
        if order.symbol != symbol {
            return true;
        }
        let hit = if order.order_type == "LIMIT" {
            match order.side {
                Side::Buy => market <= order.price,
                Side::Sell => market >= order.price,
            }
        } else if order.order_type.contains("TAKE_PROFIT") {
            match order.side {
                Side::Sell => market >= order.stop_price,
                Side::Buy => market <= order.stop_price,
            }
        } else if order.order_type.contains("STOP") {
            match order.side {
                Side::Sell => market <= order.stop_price,
                Side::Buy => market >= order.stop_price,
            }
        } else {
            false
        };
        if hit {
            triggered.push(order.clone());
        }
        !hit
    });
    let mut matched = false;
    for order in triggered {
        let (leverage, margin_mode) = snapshot
            .positions
            .iter()
            .find(|position| position.symbol == symbol)
            .map_or((fallback_leverage, fallback_margin_mode), |position| {
                (position.leverage, position.margin_mode)
            });
        // A resting limit order fills at its own price as maker; stop and
        // take-profit orders trigger a market order and pay taker.
        let limit = order.order_type == "LIMIT";
        let fee_rate = if limit {
            snapshot.commission.maker
        } else {
            snapshot.commission.taker
        };
        if simulated_fill_with_fee(
            snapshot,
            symbol,
            order.side,
            order.quantity,
            if limit { order.price } else { market },
            leverage,
            margin_mode,
            order.reduce_only,
            fee_rate,
        )
        .is_ok()
        {
            matched = true;
        } else {
            snapshot.open_orders.push(order);
        }
    }
    if !snapshot
        .positions
        .iter()
        .any(|position| position.symbol == symbol)
    {
        snapshot.open_orders.retain(|order| order.symbol != symbol);
    }
    matched
}

fn parse_input(value: &str, name: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| format!("{name} must be greater than zero."))
}

fn optional_price(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn floor_to_step(value: f64, step: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 {
        value
    } else {
        (value / step).floor() * step
    }
}

fn quantity_from_allocation(
    available: f64,
    allocation_percent: u8,
    leverage: u16,
    price: f64,
    step: f64,
) -> Option<f64> {
    (available.is_finite() && available >= 0.0 && price.is_finite() && price > 0.0).then(|| {
        floor_to_step(
            available * f64::from(allocation_percent.min(100)) / 100.0 * f64::from(leverage.max(1))
                / price,
            step,
        )
    })
}

fn round_to_step(value: f64, step: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 {
        value
    } else {
        (value / step).round() * step
    }
}

fn net_risk_levels(
    break_even: f64,
    risk_per_unit: f64,
    reward_ratio: f64,
    exit_fee: f64,
    is_long: bool,
) -> (f64, f64) {
    if is_long {
        (
            (break_even - risk_per_unit) / (1.0 - exit_fee),
            (break_even + risk_per_unit * reward_ratio) / (1.0 - exit_fee),
        )
    } else {
        (
            (break_even + risk_per_unit) / (1.0 + exit_fee),
            (break_even - risk_per_unit * reward_ratio) / (1.0 + exit_fee),
        )
    }
}

fn validate_protection(
    is_long: bool,
    reference: f64,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
) -> Result<(), String> {
    if is_long {
        if stop_loss.is_some_and(|price| price >= reference) {
            return Err(format!(
                "Long stop loss must be below the reference price {reference}."
            ));
        }
        if take_profit.is_some_and(|price| price <= reference) {
            return Err(format!(
                "Long take profit must be above the reference price {reference}."
            ));
        }
    } else {
        if stop_loss.is_some_and(|price| price <= reference) {
            return Err(format!(
                "Short stop loss must be above the reference price {reference}."
            ));
        }
        if take_profit.is_some_and(|price| price >= reference) {
            return Err(format!(
                "Short take profit must be below the reference price {reference}."
            ));
        }
    }
    Ok(())
}

fn preserve_position_costs(previous: &AccountSnapshot, current: &mut AccountSnapshot) {
    for position in &mut current.positions {
        if position.opened_at_ms.is_some() {
            continue;
        }
        let Some(old) = previous.positions.iter().find(|old| {
            old.symbol == position.symbol
                && old.side == position.side
                && (old.quantity - position.quantity).abs() < 1e-10
                && (old.entry_price - position.entry_price).abs() < 1e-10
        }) else {
            continue;
        };
        position.opened_at_ms = old.opened_at_ms;
        position.funding = old.funding;
        position.economic_break_even_price = old.economic_break_even_price;
    }
}

#[derive(Clone)]
pub struct BinanceClient {
    http: reqwest::Client,
    credentials: TradingCredentials,
    base_url: &'static str,
    recv_window_ms: u16,
    time_offset_ms: i64,
}

impl std::fmt::Debug for BinanceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinanceClient")
            .field("base_url", &self.base_url)
            .field("recv_window_ms", &self.recv_window_ms)
            .field("time_offset_ms", &self.time_offset_ms)
            .finish_non_exhaustive()
    }
}

impl BinanceClient {
    pub fn new(config: &TradingConfig, credentials: TradingCredentials) -> Result<Self, String> {
        Self::new_with_proxy(config, credentials, None)
    }

    pub fn new_with_proxy(
        config: &TradingConfig,
        credentials: TradingCredentials,
        proxy: Option<&exchange::proxy::Proxy>,
    ) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
        if let Some(proxy) = proxy {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy.to_url_string())
                    .map_err(|error| format!("Invalid trading proxy: {error}"))?,
            );
        }
        let http = builder
            .build()
            .map_err(|error| format!("Failed to initialize Binance client: {error}"))?;
        Ok(Self {
            http,
            credentials,
            base_url: if config.testnet {
                TESTNET_URL
            } else {
                MAINNET_URL
            },
            recv_window_ms: config.recv_window_ms.clamp(1_000, 60_000),
            time_offset_ms: 0,
        })
    }

    pub async fn connect(mut self, symbol: &str) -> Result<(Self, AccountSnapshot), String> {
        let server: ServerTime = self.public_get("/fapi/v1/time", &[]).await?;
        self.time_offset_ms = server.server_time as i64 - local_time_ms() as i64;
        let snapshot = self.account_snapshot(symbol).await?;
        Ok((self, snapshot))
    }

    pub async fn account_snapshot(&self, symbol: &str) -> Result<AccountSnapshot, String> {
        self.account_snapshot_inner(symbol, true, false, CommissionRate::default())
            .await
    }

    pub async fn account_snapshot_fast(
        &self,
        symbol: &str,
        hedge_mode: bool,
        commission: CommissionRate,
    ) -> Result<AccountSnapshot, String> {
        self.account_snapshot_inner(symbol, false, hedge_mode, commission)
            .await
    }

    async fn account_snapshot_inner(
        &self,
        symbol: &str,
        include_costs: bool,
        cached_hedge_mode: bool,
        cached_commission: CommissionRate,
    ) -> Result<AccountSnapshot, String> {
        let account: RawAccount = match self.signed(Method::GET, "/fapi/v3/account", vec![]).await {
            Ok(account) => account,
            Err(_) => self.signed(Method::GET, "/fapi/v2/account", vec![]).await?,
        };
        let raw_positions: Vec<RawPosition> = match self
            .signed(Method::GET, "/fapi/v3/positionRisk", vec![])
            .await
        {
            Ok(positions) => positions,
            Err(_) => {
                self.signed(Method::GET, "/fapi/v2/positionRisk", vec![])
                    .await?
            }
        };
        let open_orders: Vec<RawOpenOrder> = self
            .signed(Method::GET, "/fapi/v1/openOrders", vec![])
            .await?;
        let algo_orders = self.open_algo_orders().await.unwrap_or_else(|error| {
            log::debug!("Binance algo orders unavailable: {error}");
            Vec::new()
        });
        let hedge_mode = if include_costs {
            self.signed::<RawHedgeMode>(Method::GET, "/fapi/v1/positionSide/dual", vec![])
                .await?
                .dual_side_position
        } else {
            cached_hedge_mode
        };

        let symbol = normalize_symbol(symbol);
        let commission = if !include_costs {
            cached_commission
        } else if symbol.is_empty() {
            CommissionRate::default()
        } else {
            self.commission_rate(&symbol).await?
        };
        let current_symbol_trades = if include_costs && !symbol.is_empty() {
            Some(self.user_trades(&symbol).await?)
        } else {
            None
        };
        let mut positions = Vec::new();
        for raw in raw_positions {
            let quantity = parse_number(&raw.position_amt)?;
            if quantity.abs() <= f64::EPSILON {
                continue;
            }
            let side = parse_position_side(&raw.position_side)?;
            let costs = if include_costs {
                let other_trades;
                let trades = if raw.symbol == symbol {
                    current_symbol_trades.as_deref().unwrap_or_default()
                } else {
                    other_trades = self.user_trades(&raw.symbol).await?;
                    &other_trades
                };
                current_position_costs(quantity, side, trades)
            } else {
                PositionCosts::default()
            };
            let opened_at_ms = costs.opened_at_ms;
            let funding = if include_costs {
                self.funding_income(&raw.symbol, opened_at_ms).await?
            } else {
                0.0
            };
            let break_even_price = parse_number(&raw.break_even_price)
                .unwrap_or_else(|_| parse_number(&raw.entry_price).unwrap_or_default());
            let entry_price = parse_number(&raw.entry_price)?;
            let economic_break_even_price = if quantity.abs() > f64::EPSILON {
                entry_price - (costs.realized_after_commission + funding) / quantity
            } else {
                entry_price
            };
            positions.push(Position {
                symbol: raw.symbol,
                side,
                quantity,
                entry_price,
                mark_price: parse_number(&raw.mark_price)?,
                break_even_price,
                economic_break_even_price,
                liquidation_price: parse_number(&raw.liquidation_price).unwrap_or_default(),
                leverage: raw.leverage.parse().unwrap_or(1),
                margin_mode: if raw.margin_type.eq_ignore_ascii_case("isolated") {
                    MarginMode::Isolated
                } else {
                    MarginMode::Cross
                },
                unrealized_pnl: parse_number(&raw.unrealized_profit).unwrap_or_default(),
                funding,
                opened_at_ms,
            });
        }

        let trade_history = current_symbol_trades
            .as_ref()
            .map(|trades| {
                trades
                    .iter()
                    .filter_map(|trade| trade_history_from_binance(&symbol, trade))
                    .collect()
            })
            .unwrap_or_default();

        Ok(AccountSnapshot {
            balances: account
                .assets
                .into_iter()
                .filter_map(|asset| {
                    let wallet = parse_number(&asset.wallet_balance).ok()?;
                    let available = parse_number(&asset.available_balance).ok()?;
                    let unrealized_pnl = parse_number(&asset.unrealized_profit).ok()?;
                    ((wallet.abs() + available.abs() + unrealized_pnl.abs()) > f64::EPSILON)
                        .then_some(Balance {
                            asset: asset.asset,
                            wallet,
                            available,
                            unrealized_pnl,
                        })
                })
                .collect(),
            positions,
            trade_history,
            event_contracts: Vec::new(),
            open_orders: open_orders
                .into_iter()
                .map(|order| OpenOrder {
                    symbol: order.symbol,
                    order_id: order.order_id,
                    side: if order.side == "BUY" {
                        Side::Buy
                    } else {
                        Side::Sell
                    },
                    order_type: order.order_type,
                    price: parse_number(&order.price).unwrap_or_default(),
                    stop_price: parse_number(&order.stop_price).unwrap_or_default(),
                    quantity: parse_number(&order.orig_qty).unwrap_or_default(),
                    reduce_only: order.reduce_only,
                    is_algo: false,
                })
                .chain(algo_orders)
                .collect(),
            commission,
            liquidation_fee_rate: 0.0,
            hedge_mode,
            updated_at_ms: local_time_ms(),
        })
    }

    pub async fn set_leverage(&self, symbol: &str, leverage: u16) -> Result<(), String> {
        let leverage = leverage.clamp(1, 125);
        let _: serde_json::Value = self
            .signed(
                Method::POST,
                "/fapi/v1/leverage",
                vec![
                    ("symbol", normalize_symbol(symbol)),
                    ("leverage", leverage.to_string()),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn set_margin_mode(&self, symbol: &str, mode: MarginMode) -> Result<(), String> {
        let result: Result<serde_json::Value, String> = self
            .signed(
                Method::POST,
                "/fapi/v1/marginType",
                vec![
                    ("symbol", normalize_symbol(symbol)),
                    ("marginType", mode.api().to_string()),
                ],
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if error.contains("No need to change margin type") => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn place_order(&self, request: OrderRequest) -> Result<PlacedOrder, String> {
        request.validate()?;
        let symbol = normalize_symbol(&request.symbol);
        let mut params = vec![
            ("symbol", symbol.clone()),
            ("side", request.side.api().to_string()),
            ("positionSide", request.position_side.api().to_string()),
            ("quantity", format_decimal(request.quantity)),
            (
                "newClientOrderId",
                format!("fs_{}", uuid::Uuid::new_v4().simple()),
            ),
            ("newOrderRespType", "RESULT".to_string()),
        ];
        match request.order_type {
            OrderType::Market => params.push(("type", "MARKET".to_string())),
            OrderType::Limit => {
                params.push(("type", "LIMIT".to_string()));
                params.push(("timeInForce", "GTC".to_string()));
                params.push((
                    "price",
                    format_decimal(request.price.expect("validated limit price")),
                ));
            }
        }
        if request.reduce_only && request.position_side == PositionSide::Both {
            params.push(("reduceOnly", "true".to_string()));
        }
        let raw: RawPlacedOrder = self.signed(Method::POST, "/fapi/v1/order", params).await?;

        if !request.reduce_only {
            if let Some(stop_price) = request.stop_loss {
                self.place_protective_order(
                    &symbol,
                    request.side,
                    request.position_side,
                    "STOP_MARKET",
                    stop_price,
                )
                .await
                .map_err(|error| {
                    format!(
                        "Entry order {} was accepted, but stop loss placement failed: {error}",
                        raw.order_id
                    )
                })?;
            }
            if let Some(stop_price) = request.take_profit {
                self.place_protective_order(
                    &symbol,
                    request.side,
                    request.position_side,
                    "TAKE_PROFIT_MARKET",
                    stop_price,
                )
                .await
                .map_err(|error| {
                    format!(
                        "Entry order {} was accepted, but take profit placement failed: {error}",
                        raw.order_id
                    )
                })?;
            }
        }

        Ok(PlacedOrder {
            symbol: raw.symbol,
            order_id: raw.order_id,
            status: raw.status,
        })
    }

    async fn place_protective_order(
        &self,
        symbol: &str,
        entry_side: Side,
        position_side: PositionSide,
        order_type: &str,
        stop_price: f64,
    ) -> Result<(), String> {
        let close_side = match entry_side {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        };
        let legacy: Result<RawPlacedOrder, String> = self
            .signed(
                Method::POST,
                "/fapi/v1/order",
                vec![
                    ("symbol", symbol.to_string()),
                    ("side", close_side.api().to_string()),
                    ("positionSide", position_side.api().to_string()),
                    ("type", order_type.to_string()),
                    ("stopPrice", format_decimal(stop_price)),
                    ("closePosition", "true".to_string()),
                    ("workingType", "MARK_PRICE".to_string()),
                    ("priceProtect", "true".to_string()),
                    (
                        "newClientOrderId",
                        format!("fs_{}", uuid::Uuid::new_v4().simple()),
                    ),
                ],
            )
            .await;
        let legacy_error = match legacy {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };

        let _: serde_json::Value = self
            .signed(
                Method::POST,
                "/fapi/v1/algoOrder",
                vec![
                    ("algoType", "CONDITIONAL".to_string()),
                    ("symbol", symbol.to_string()),
                    ("side", close_side.api().to_string()),
                    ("positionSide", position_side.api().to_string()),
                    ("type", order_type.to_string()),
                    ("triggerPrice", format_decimal(stop_price)),
                    ("closePosition", "true".to_string()),
                    ("workingType", "MARK_PRICE".to_string()),
                    ("priceProtect", "true".to_string()),
                    (
                        "clientAlgoId",
                        format!("fs_{}", uuid::Uuid::new_v4().simple()),
                    ),
                ],
            )
            .await
            .map_err(|algo_error| {
                format!(
                    "Conditional order failed on both Binance order APIs: {}; {algo_error}",
                    legacy_error
                )
            })?;
        Ok(())
    }

    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: u64,
        is_algo: bool,
    ) -> Result<(), String> {
        let _: serde_json::Value = self
            .signed(
                Method::DELETE,
                if is_algo {
                    "/fapi/v1/algoOrder"
                } else {
                    "/fapi/v1/order"
                },
                vec![
                    ("symbol", normalize_symbol(symbol)),
                    (
                        if is_algo { "algoId" } else { "orderId" },
                        order_id.to_string(),
                    ),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn modify_limit_order(&self, order: &OpenOrder, price: f64) -> Result<(), String> {
        if order.is_algo || order.order_type != "LIMIT" {
            return Err("Only regular Binance limit orders can be dragged.".to_string());
        }
        let _: serde_json::Value = self
            .signed(
                Method::PUT,
                "/fapi/v1/order",
                vec![
                    ("symbol", normalize_symbol(&order.symbol)),
                    ("orderId", order.order_id.to_string()),
                    ("side", order.side.api().to_string()),
                    ("quantity", format_decimal(order.quantity)),
                    ("price", format_decimal(price)),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<(), String> {
        let symbol = normalize_symbol(symbol);
        let _: serde_json::Value = self
            .signed(
                Method::DELETE,
                "/fapi/v1/allOpenOrders",
                vec![("symbol", symbol.clone())],
            )
            .await?;
        let algo: Result<serde_json::Value, String> = self
            .signed(
                Method::DELETE,
                "/fapi/v1/allOpenAlgoOrders",
                vec![("symbol", symbol), ("algoType", "CONDITIONAL".to_string())],
            )
            .await;
        if let Err(error) = algo {
            log::debug!("Binance cancel-all algo endpoint unavailable: {error}");
        }
        Ok(())
    }

    async fn open_algo_orders(&self) -> Result<Vec<OpenOrder>, String> {
        let value: serde_json::Value = self
            .signed(
                Method::GET,
                "/fapi/v1/openAlgoOrders",
                vec![("algoType", "CONDITIONAL".to_string())],
            )
            .await?;
        let rows = value
            .get("orders")
            .and_then(serde_json::Value::as_array)
            .or_else(|| value.as_array())
            .cloned()
            .unwrap_or_default();
        rows.into_iter()
            .map(|row| {
                let order: RawAlgoOrder = serde_json::from_value(row)
                    .map_err(|error| format!("Failed to parse Binance algo order: {error}"))?;
                Ok(OpenOrder {
                    symbol: order.symbol,
                    order_id: order.algo_id,
                    side: if order.side == "BUY" {
                        Side::Buy
                    } else {
                        Side::Sell
                    },
                    order_type: order.order_type,
                    price: parse_number(&order.price).unwrap_or_default(),
                    stop_price: parse_number(&order.trigger_price).unwrap_or_default(),
                    quantity: parse_number(&order.quantity).unwrap_or_default(),
                    reduce_only: order.close_position,
                    is_algo: true,
                })
            })
            .collect()
    }

    pub async fn close_position(&self, position: &Position) -> Result<PlacedOrder, String> {
        self.place_order(OrderRequest {
            symbol: position.symbol.clone(),
            side: if position.is_long() {
                Side::Sell
            } else {
                Side::Buy
            },
            position_side: position.side,
            order_type: OrderType::Market,
            quantity: position.quantity.abs(),
            price: None,
            reduce_only: position.side == PositionSide::Both,
            stop_loss: None,
            take_profit: None,
        })
        .await
    }

    async fn commission_rate(&self, symbol: &str) -> Result<CommissionRate, String> {
        let raw: RawCommissionRate = self
            .signed(
                Method::GET,
                "/fapi/v1/commissionRate",
                vec![("symbol", symbol.to_string())],
            )
            .await?;
        Ok(CommissionRate {
            maker: parse_number(&raw.maker_commission_rate)?,
            taker: parse_number(&raw.taker_commission_rate)?,
        })
    }

    async fn mark_price(&self, symbol: &str) -> Result<f64, String> {
        let raw: RawMarkPrice = self
            .public_get(
                "/fapi/v1/premiumIndex",
                &[("symbol".to_string(), normalize_symbol(symbol))],
            )
            .await?;
        parse_number(&raw.mark_price)
    }

    async fn user_trades(&self, symbol: &str) -> Result<Vec<RawUserTrade>, String> {
        self.signed(
            Method::GET,
            "/fapi/v1/userTrades",
            vec![
                ("symbol", symbol.to_string()),
                ("limit", "1000".to_string()),
            ],
        )
        .await
    }

    async fn funding_income(&self, symbol: &str, start_ms: Option<u64>) -> Result<f64, String> {
        const SEVEN_DAYS_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
        const NINETY_DAYS_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
        let now = self.timestamp_ms();
        let Some(mut cursor) = start_ms.map(|value| value.max(now.saturating_sub(NINETY_DAYS_MS)))
        else {
            let rows: Vec<RawIncome> = self
                .signed(
                    Method::GET,
                    "/fapi/v1/income",
                    vec![
                        ("symbol", symbol.to_string()),
                        ("incomeType", "FUNDING_FEE".to_string()),
                        ("limit", "1000".to_string()),
                    ],
                )
                .await?;
            return sum_income(rows);
        };
        let mut total = 0.0;
        while cursor <= now {
            let end = cursor.saturating_add(SEVEN_DAYS_MS - 1).min(now);
            let rows: Vec<RawIncome> = self
                .signed(
                    Method::GET,
                    "/fapi/v1/income",
                    vec![
                        ("symbol", symbol.to_string()),
                        ("incomeType", "FUNDING_FEE".to_string()),
                        ("startTime", cursor.to_string()),
                        ("endTime", end.to_string()),
                        ("limit", "1000".to_string()),
                    ],
                )
                .await?;
            total += sum_income(rows)?;
            if end == now {
                break;
            }
            cursor = end + 1;
        }
        Ok(total)
    }

    async fn public_get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        params: &[(String, String)],
    ) -> Result<T, String> {
        let query = encode_params(params);
        let url = if query.is_empty() {
            format!("{}{path}", self.base_url)
        } else {
            format!("{}{path}?{query}", self.base_url)
        };
        let response = self.http.get(url).send().await.map_err(network_error)?;
        decode_response(response).await
    }

    async fn signed<T: for<'de> Deserialize<'de>>(
        &self,
        method: Method,
        path: &str,
        mut params: Vec<(&str, String)>,
    ) -> Result<T, String> {
        params.push(("recvWindow", self.recv_window_ms.to_string()));
        params.push(("timestamp", self.timestamp_ms().to_string()));
        let owned = params
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<Vec<_>>();
        let query = encode_params(&owned);
        let signature = sign(&self.credentials.secret_key, &query)?;
        let url = format!("{}{path}?{query}&signature={signature}", self.base_url);
        let response = self
            .http
            .request(method, url)
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .send()
            .await
            .map_err(network_error)?;
        decode_response(response).await
    }

    fn timestamp_ms(&self) -> u64 {
        if self.time_offset_ms >= 0 {
            local_time_ms().saturating_add(self.time_offset_ms as u64)
        } else {
            local_time_ms().saturating_sub(self.time_offset_ms.unsigned_abs())
        }
    }
}

async fn fetch_binance_liquidation_fee(
    symbol: String,
    proxy: Option<exchange::proxy::Proxy>,
) -> Result<f64, String> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
    if let Some(proxy) = proxy {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy.to_url_string())
                .map_err(|error| format!("Invalid trading proxy: {error}"))?,
        );
    }
    let response = builder
        .build()
        .map_err(|error| format!("Failed to initialize Binance client: {error}"))?
        .get(format!("{MAINNET_URL}/fapi/v1/exchangeInfo"))
        .send()
        .await
        .map_err(network_error)?;
    let exchange: RawExchangeInfo = decode_response(response).await?;
    let normalized = normalize_symbol(&symbol);
    let rate = exchange
        .symbols
        .into_iter()
        .find(|rules| rules.symbol == normalized)
        .ok_or_else(|| format!("Binance exchange info has no {normalized} rules."))?
        .liquidation_fee;
    parse_number(&rate)
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        tr("Binance request timed out.").to_string()
    } else {
        format!("Binance request failed: {error}")
    }
}

async fn decode_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read Binance response: {error}"))?;
    if !status.is_success() {
        let error = serde_json::from_str::<BinanceApiError>(&body).ok();
        return Err(error.map_or_else(
            || format!("Binance returned HTTP {status}: {body}"),
            |error| format!("Binance error {}: {}", error.code, error.msg),
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse Binance response: {error}"))
}

fn sign(secret: &str, payload: &str) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| "Invalid Binance secret key.".to_string())?;
    mac.update(payload.as_bytes());
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn encode_params(params: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

fn local_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn normalize_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn format_decimal(value: f64) -> String {
    let value = format!("{value:.8}");
    value
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn format_price_2(value: f64) -> String {
    format!("{value:.2}")
}

fn parse_number(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|_| format!("Invalid number in Binance response: {value}"))
}

fn sum_income(rows: Vec<RawIncome>) -> Result<f64, String> {
    rows.into_iter()
        .map(|row| parse_number(&row.income))
        .try_fold(0.0, |sum, value| value.map(|value| sum + value))
}

fn parse_position_side(value: &str) -> Result<PositionSide, String> {
    match value {
        "BOTH" => Ok(PositionSide::Both),
        "LONG" => Ok(PositionSide::Long),
        "SHORT" => Ok(PositionSide::Short),
        _ => Err(format!("Unsupported Binance position side: {value}")),
    }
}

fn trade_history_from_binance(symbol: &str, trade: &RawUserTrade) -> Option<TradeHistory> {
    let quantity = parse_number(&trade.qty).ok()?;
    let price = parse_number(&trade.price).ok()?;
    Some(TradeHistory {
        trade_id: trade.id,
        order_id: trade.order_id,
        symbol: if trade.symbol.is_empty() {
            symbol.to_string()
        } else {
            trade.symbol.clone()
        },
        side: if trade.side == "BUY" {
            Side::Buy
        } else {
            Side::Sell
        },
        quantity,
        price,
        realized_pnl: parse_number(&trade.realized_pnl).unwrap_or_default(),
        commission: parse_number(&trade.commission).unwrap_or_default(),
        commission_asset: if trade.commission_asset.is_empty() {
            "USDT".to_string()
        } else {
            trade.commission_asset.clone()
        },
        time_ms: trade.time,
    })
}

#[derive(Default)]
struct PositionCosts {
    opened_at_ms: Option<u64>,
    realized_after_commission: f64,
}

fn current_position_costs(
    current_quantity: f64,
    position_side: PositionSide,
    trades: &[RawUserTrade],
) -> PositionCosts {
    let mut quantity = current_quantity;
    let sign = current_quantity.signum();
    let matching = trades
        .iter()
        .filter(|trade| trade.position_side == position_side.api())
        .collect::<Vec<_>>();
    let mut cycle_start = matching.len();
    for (index, trade) in matching.iter().enumerate().rev() {
        let Ok(trade_quantity) = parse_number(&trade.qty) else {
            return PositionCosts::default();
        };
        let signed_trade = if trade.side == "BUY" {
            trade_quantity
        } else {
            -trade_quantity
        };
        let before = quantity - signed_trade;
        cycle_start = index;
        if before.abs() <= 1e-10 || before.signum() != sign {
            break;
        }
        quantity = before;
    }
    let cycle = &matching[cycle_start..];
    PositionCosts {
        opened_at_ms: cycle.first().map(|trade| trade.time),
        realized_after_commission: cycle
            .iter()
            .map(|trade| {
                parse_number(&trade.realized_pnl).unwrap_or_default()
                    - parse_number(&trade.commission).unwrap_or_default()
            })
            .sum(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerTime {
    server_time: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMarkPrice {
    mark_price: String,
}

#[derive(Deserialize)]
struct RawExchangeInfo {
    symbols: Vec<RawSymbolRules>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSymbolRules {
    symbol: String,
    liquidation_fee: String,
}

#[derive(Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAccount {
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAsset {
    asset: String,
    wallet_balance: String,
    available_balance: String,
    #[serde(default)]
    unrealized_profit: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPosition {
    symbol: String,
    position_amt: String,
    entry_price: String,
    #[serde(default)]
    break_even_price: String,
    mark_price: String,
    #[serde(default, alias = "unRealizedProfit")]
    unrealized_profit: String,
    liquidation_price: String,
    leverage: String,
    margin_type: String,
    position_side: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpenOrder {
    symbol: String,
    order_id: u64,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    price: String,
    stop_price: String,
    orig_qty: String,
    #[serde(default)]
    reduce_only: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAlgoOrder {
    symbol: String,
    algo_id: u64,
    side: String,
    #[serde(alias = "type")]
    order_type: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    trigger_price: String,
    #[serde(default)]
    quantity: String,
    #[serde(default)]
    close_position: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHedgeMode {
    dual_side_position: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommissionRate {
    maker_commission_rate: String,
    taker_commission_rate: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUserTrade {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    order_id: u64,
    #[serde(default)]
    price: String,
    qty: String,
    side: String,
    position_side: String,
    time: u64,
    #[serde(default)]
    commission: String,
    #[serde(default)]
    commission_asset: String,
    #[serde(default)]
    realized_pnl: String,
}

#[derive(Deserialize)]
struct RawIncome {
    income: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlacedOrder {
    symbol: String,
    order_id: u64,
    status: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn order_ticket_survives_restart() {
        let mut trader = super::Trader::new(TradingConfig::default(), None);
        trader.price = "61234.5".to_string();
        trader.stop_loss = "60000".to_string();
        trader.event_stake = "75".to_string();
        trader.event_duration = EventDuration::H1;
        trader.order_type = OrderType::Limit;
        let json = serde_json::to_string(&trader.config()).unwrap();
        let restored = super::Trader::new(serde_json::from_str(&json).unwrap(), None);
        assert_eq!(restored.price, "61234.5");
        assert_eq!(restored.stop_loss, "60000");
        assert_eq!(restored.event_stake, "75");
        assert_eq!(restored.event_duration, EventDuration::H1);
        assert_eq!(restored.order_type, OrderType::Limit);
    }

    use super::*;

    fn trade(time: u64, side: &str, qty: &str) -> RawUserTrade {
        RawUserTrade {
            symbol: "BTCUSDT".to_string(),
            id: time,
            order_id: time,
            price: "100".to_string(),
            qty: qty.to_string(),
            side: side.to_string(),
            position_side: "BOTH".to_string(),
            time,
            commission: "0.1".to_string(),
            commission_asset: "USDT".to_string(),
            realized_pnl: "0".to_string(),
        }
    }

    #[test]
    fn allocation_quantity_uses_available_balance_and_leverage() {
        assert_eq!(
            quantity_from_allocation(1_000.0, 25, 10, 50_000.0, 0.001),
            Some(0.05)
        );
        assert_eq!(quantity_from_allocation(1_000.0, 25, 10, 0.0, 0.001), None);
    }

    #[test]
    fn account_refresh_does_not_overwrite_entered_quantity() {
        let mut trader = Trader::new(
            TradingConfig {
                exchange: TradingExchange::Simulation,
                ..TradingConfig::default()
            },
            None,
        );
        trader.set_chart_sources(vec![ChartSource {
            pane_id: uuid::Uuid::new_v4(),
            link_group: None,
            binance_usdm: true,
            label: "BTCUSDT".to_string(),
            symbol: "BTCUSDT".to_string(),
            min_tick: 0.1,
            min_qty: 0.001,
            latest_price: Some(100.0),
            market_latency_ms: None,
            best_bid: None,
            best_ask: None,
        }]);
        trader.quantity = "1".to_string();

        let _ = trader.update(Message::SnapshotLoaded(Ok(simulated_snapshot(9_000))));

        assert_eq!(trader.quantity, "1");
    }

    #[test]
    fn simulated_liquidation_price_includes_liquidation_fee_reserve() {
        let long = estimated_liquidation_price(100.0, 10, true, 0.005, 0.0125).unwrap();
        let long_without_fee = estimated_liquidation_price(100.0, 10, true, 0.005, 0.0).unwrap();
        let short = estimated_liquidation_price(100.0, 10, false, 0.005, 0.0125).unwrap();

        assert!(long > long_without_fee && long < 100.0);
        assert!(short > 100.0 && short < 110.0);

        // High leverage: always on the losing side of entry, inside the margin.
        for leverage in [50, 58, 75, 100, 125] {
            let long = estimated_liquidation_price(100.0, leverage, true, 0.005, 0.0125).unwrap();
            let short = estimated_liquidation_price(100.0, leverage, false, 0.005, 0.0125).unwrap();
            let margin = 100.0 / f64::from(leverage);
            assert!(
                long < 100.0 && long > 100.0 - margin,
                "{leverage}x long {long}"
            );
            assert!(
                short > 100.0 && short < 100.0 + margin,
                "{leverage}x short {short}"
            );
        }
    }

    #[test]
    fn cross_liquidation_uses_the_whole_wallet() {
        let open = |wallet: u64, side, mode| {
            let mut snapshot = simulated_snapshot(wallet);
            simulated_fill(&mut snapshot, "BTCUSDT", side, 1.0, 100.0, 10, mode, false).unwrap();
            refresh_simulated_account(&mut snapshot, "BTCUSDT", 100.0);
            snapshot
        };
        // A wallet far larger than the position: no liquidation at all.
        let rich = open(10_000, Side::Buy, MarginMode::Cross);
        assert_eq!(rich.positions[0].liquidation_price, 0.0);

        // Wallet just above the margin: cross sits below isolated for a long,
        // above it for a short, always on the losing side of entry.
        let long = open(20, Side::Buy, MarginMode::Cross).positions[0].liquidation_price;
        let isolated = open(20, Side::Buy, MarginMode::Isolated).positions[0].liquidation_price;
        assert!(
            long > 0.0 && long < isolated && isolated < 100.0,
            "{long} {isolated}"
        );
        let short = open(20, Side::Sell, MarginMode::Cross).positions[0].liquidation_price;
        let isolated = open(20, Side::Sell, MarginMode::Isolated).positions[0].liquidation_price;
        assert!(short > isolated && isolated > 100.0, "{short} {isolated}");

        // At the cross price, equity equals the maintenance reserve.
        let snapshot = open(20, Side::Buy, MarginMode::Cross);
        let wallet = snapshot.balances[0].wallet;
        let rate = maintenance_reserve_rate(10, SIM_MAINTENANCE_MARGIN_RATE, 0.0125);
        assert!((wallet + (long - 100.0) - long * rate).abs() < 1e-9);
    }

    #[test]
    fn long_and_short_actions_map_to_open_and_reduce_only_sides() {
        assert_eq!(
            trade_actions(PositionSide::Long),
            [
                (Side::Buy, false, "Buy / Open"),
                (Side::Sell, true, "Sell / Close"),
            ]
        );
        assert_eq!(
            trade_actions(PositionSide::Short),
            [
                (Side::Sell, false, "Sell / Open"),
                (Side::Buy, true, "Buy / Close"),
            ]
        );
    }

    #[test]
    fn reduce_only_orders_ignore_attached_entry_protection() {
        let mut trader = Trader::new(
            TradingConfig {
                exchange: TradingExchange::Simulation,
                ..TradingConfig::default()
            },
            None,
        );
        trader.quantity = "1".to_string();
        trader.stop_loss = "95".to_string();
        trader.take_profit = "110".to_string();
        trader.reduce_only = true;

        let request = trader.order_request().unwrap();

        assert!(request.reduce_only);
        assert_eq!(request.stop_loss, None);
        assert_eq!(request.take_profit, None);
    }

    #[test]
    fn account_performance_uses_only_realized_fills_for_win_rate() {
        let history = [
            TradeHistory {
                trade_id: 1,
                order_id: 1,
                symbol: "BTCUSDT".to_string(),
                side: Side::Buy,
                quantity: 1.0,
                price: 100.0,
                realized_pnl: 0.0,
                commission: 0.1,
                commission_asset: "USDT".to_string(),
                time_ms: 1,
            },
            TradeHistory {
                trade_id: 2,
                order_id: 2,
                symbol: "BTCUSDT".to_string(),
                side: Side::Sell,
                quantity: 0.5,
                price: 110.0,
                realized_pnl: 10.0,
                commission: 0.1,
                commission_asset: "USDT".to_string(),
                time_ms: 2,
            },
            TradeHistory {
                trade_id: 3,
                order_id: 3,
                symbol: "BTCUSDT".to_string(),
                side: Side::Sell,
                quantity: 0.5,
                price: 96.0,
                realized_pnl: -4.0,
                commission: 0.1,
                commission_asset: "USDT".to_string(),
                time_ms: 3,
            },
        ];

        let analytics = account_analytics(&history);

        assert_eq!(analytics.closed_fills, 2);
        assert_eq!((analytics.wins, analytics.losses), (1, 1));
        assert!((analytics.win_rate() - 50.0).abs() < 1e-9);
        assert!((analytics.net_pnl - 5.7).abs() < 1e-9);
    }

    #[test]
    fn simulation_fill_updates_balance_position_and_limit_orders() {
        let mut snapshot = simulated_snapshot(10_000);
        simulated_fill(
            &mut snapshot,
            "BTCUSDT",
            Side::Buy,
            1.0,
            100.0,
            10,
            MarginMode::Cross,
            false,
        )
        .unwrap();
        assert_eq!(snapshot.positions[0].quantity, 1.0);
        assert_eq!(snapshot.trade_history.len(), 1);
        refresh_simulated_account(&mut snapshot, "BTCUSDT", 110.0);
        assert_eq!(snapshot.positions[0].unrealized_pnl, 10.0);
        // Cross: a 10k wallet behind a 100 USDT position never liquidates.
        assert_eq!(snapshot.positions[0].liquidation_price, 0.0);

        snapshot.open_orders.push(OpenOrder {
            symbol: "BTCUSDT".to_string(),
            order_id: 1,
            side: Side::Sell,
            order_type: "LIMIT".to_string(),
            price: 120.0,
            stop_price: 0.0,
            quantity: 1.0,
            reduce_only: true,
            is_algo: false,
        });
        assert!(!match_simulated_orders(
            &mut snapshot,
            "BTCUSDT",
            119.0,
            10,
            MarginMode::Cross
        ));
        assert!(match_simulated_orders(
            &mut snapshot,
            "BTCUSDT",
            120.0,
            10,
            MarginMode::Cross
        ));
        assert!(snapshot.positions.is_empty());
        assert!(snapshot.balances[0].wallet > 10_019.0);
        assert_eq!(snapshot.trade_history.len(), 2);
        assert!(snapshot.trade_history[1].realized_pnl > 0.0);
    }

    #[test]
    fn simulation_break_even_includes_fees() {
        let mut snapshot = simulated_snapshot(10_000);
        let taker = snapshot.commission.taker;
        let buy = |snapshot: &mut AccountSnapshot, side, quantity, price| {
            simulated_fill(
                snapshot,
                "BTCUSDT",
                side,
                quantity,
                price,
                10,
                MarginMode::Cross,
                false,
            )
            .unwrap();
        };
        buy(&mut snapshot, Side::Buy, 1.0, 100.0);
        let position = &snapshot.positions[0];
        assert!((position.economic_break_even_price - 100.0 * (1.0 + taker)).abs() < 1e-9);
        // Net PnL at break-even, before the exit fee, is zero.
        assert!(
            position
                .estimated_pnl_at(position.economic_break_even_price, 0.0)
                .abs()
                < 1e-9
        );

        // Half closed at 110: like Binance, break-even absorbs the realized
        // profit, so PnL at a price is the whole position's net since opening.
        buy(&mut snapshot, Side::Sell, 0.5, 110.0);
        let at_110 = snapshot.positions[0].estimated_pnl_at(110.0, 0.0);
        assert!((at_110 - (10.0 - 100.0 * taker - 55.0 * taker)).abs() < 1e-9);

        // Short: break-even sits below entry.
        let mut snapshot = simulated_snapshot(10_000);
        buy(&mut snapshot, Side::Sell, 1.0, 100.0);
        assert!(snapshot.positions[0].economic_break_even_price < 100.0);
    }

    #[test]
    fn reconstructs_current_position_open_time() {
        let trades = vec![
            trade(10, "BUY", "1"),
            trade(20, "SELL", "1"),
            trade(30, "BUY", "0.4"),
            trade(40, "BUY", "0.6"),
            trade(50, "SELL", "0.25"),
        ];
        let costs = current_position_costs(0.75, PositionSide::Both, &trades);
        assert_eq!(costs.opened_at_ms, Some(30));
        assert!((costs.realized_after_commission + 0.3).abs() < 1e-9);
    }

    #[test]
    fn binance_trade_history_keeps_realized_pnl_and_fees() {
        let mut raw = trade(10, "SELL", "0.5");
        raw.realized_pnl = "5".to_string();
        raw.commission = "0.1".to_string();

        let history = trade_history_from_binance("BTCUSDT", &raw).unwrap();

        assert_eq!(history.price, 100.0);
        assert!((history.net_pnl_usdt() - 4.9).abs() < 1e-9);
    }

    #[test]
    fn economic_break_even_and_exit_fee_are_included() {
        let position = Position {
            symbol: "BTCUSDT".to_string(),
            side: PositionSide::Both,
            quantity: 2.0,
            entry_price: 100.0,
            mark_price: 100.0,
            break_even_price: 100.1,
            economic_break_even_price: 100.5,
            liquidation_price: 50.0,
            leverage: 10,
            margin_mode: MarginMode::Cross,
            unrealized_pnl: 0.0,
            funding: -0.8,
            opened_at_ms: None,
        };
        assert!((position.estimated_pnl_at(110.0, 0.001) - 18.78).abs() < 1e-9);
    }

    #[test]
    fn signing_matches_known_hmac_sha256_vector() {
        assert_eq!(
            sign("key", "The quick brown fox jumps over the lazy dog").unwrap(),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn rejects_protection_on_the_wrong_side() {
        assert!(validate_protection(true, 100.0, Some(101.0), None).is_err());
        assert!(validate_protection(false, 100.0, None, Some(101.0)).is_err());
        assert!(validate_protection(true, 100.0, Some(99.0), Some(102.0)).is_ok());
    }

    #[test]
    fn editing_protection_loads_the_positions_existing_levels() {
        let mut trader = Trader::new(
            TradingConfig {
                exchange: TradingExchange::Simulation,
                ..TradingConfig::default()
            },
            None,
        );
        let mut snapshot = simulated_snapshot(10_000);
        simulated_fill(
            &mut snapshot,
            "BTCUSDT",
            Side::Buy,
            1.0,
            100.0,
            10,
            MarginMode::Cross,
            false,
        )
        .unwrap();
        add_simulated_protection(
            &mut snapshot,
            "BTCUSDT",
            Side::Sell,
            1.0,
            Some(95.0),
            Some(110.0),
        );
        trader.snapshot = Some(snapshot);

        let _ = trader.update(Message::EditProtection(0));

        let editor = trader.protection_editor.as_ref().unwrap();
        assert_eq!(editor.symbol, "BTCUSDT");
        assert_eq!(editor.stop_loss, "95.00");
        assert_eq!(editor.take_profit, "110.00");
        assert!(trader.stop_loss.is_empty());
        assert!(trader.take_profit.is_empty());
    }

    #[test]
    fn fee_aware_levels_preserve_net_risk_reward() {
        let (stop, target) = net_risk_levels(100.0, 1.0, 2.0, 0.001, true);
        let net = |price: f64| price - 100.0 - price * 0.001;
        assert!((net(stop) + 1.0).abs() < 1e-9);
        assert!((net(target) - 2.0).abs() < 1e-9);
    }
}
