use data::config::trading::{TradingConfig, TradingCredentials};
use hmac::{Hmac, Mac};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, rule, scrollable, text, text_input,
};
use iced::{Alignment, Element, Length, Task, padding};
use reqwest::Method;
use serde::Deserialize;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

const MAINNET_URL: &str = "https://fapi.binance.com";
const TESTNET_URL: &str = "https://demo-fapi.binance.com";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Buy,
    Sell,
}

impl Side {
    pub const ALL: [Self; 2] = [Self::Buy, Self::Sell];

    fn api(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Buy => "Buy / Long",
            Self::Sell => "Sell / Short",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
            Self::Market => "Market",
            Self::Limit => "Limit",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
            Self::Cross => "Cross",
            Self::Isolated => "Isolated",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PositionSide {
    #[default]
    Both,
    Long,
    Short,
}

impl PositionSide {
    fn api(self) -> &'static str {
        match self {
            Self::Both => "BOTH",
            Self::Long => "LONG",
            Self::Short => "SHORT",
        }
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
            return Err("Symbol is required.".to_string());
        }
        if !self.quantity.is_finite() || self.quantity <= 0.0 {
            return Err("Quantity must be greater than zero.".to_string());
        }
        if self.order_type == OrderType::Limit
            && !self
                .price
                .is_some_and(|price| price.is_finite() && price > 0.0)
        {
            return Err("A valid price is required for a limit order.".to_string());
        }
        for (name, value) in [
            ("Stop loss", self.stop_loss),
            ("Take profit", self.take_profit),
        ] {
            if value.is_some_and(|price| !price.is_finite() || price <= 0.0) {
                return Err(format!("{name} must be a positive price."));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Balance {
    pub asset: String,
    pub wallet: f64,
    pub available: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Copy, Default)]
pub struct CommissionRate {
    pub maker: f64,
    pub taker: f64,
}

#[derive(Debug, Clone, Default)]
pub struct AccountSnapshot {
    pub balances: Vec<Balance>,
    pub positions: Vec<Position>,
    pub open_orders: Vec<OpenOrder>,
    pub commission: CommissionRate,
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
    pub label: String,
    pub symbol: String,
    pub min_tick: f64,
    pub min_qty: f64,
    pub latest_price: Option<f64>,
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
    pub entry_price: f64,
    pub break_even_price: f64,
    pub liquidation_price: Option<f64>,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub unrealized_pnl: f64,
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
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    ApiKeyChanged(String),
    SecretKeyChanged(String),
    ToggleTestnet(bool),
    ConfirmOrdersChanged(bool),
    SaveCredentials,
    Connect,
    Disconnect,
    Refresh,
    Poll,
    Connected(Result<(BinanceClient, AccountSnapshot), String>),
    SnapshotLoaded(Result<AccountSnapshot, String>),
    SymbolChanged(String),
    SideChanged(Side),
    OrderTypeChanged(OrderType),
    MarginModeChanged(MarginMode),
    QuantityChanged(String),
    PriceChanged(String),
    StopLossChanged(String),
    TakeProfitChanged(String),
    LeverageChanged(String),
    RiskPercentChanged(String),
    RewardRatioChanged(String),
    CalculateRiskLevels,
    ReduceOnlyChanged(bool),
    ChartSourceChanged(ChartSource),
    PlaceOrder,
    CancelOrder(String, u64, bool),
    CancelAllOrders,
    ClosePosition(usize),
    ApplyProtection(usize),
    ConfirmAction,
    CancelAction,
    ActionCompleted(Result<String, String>),
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
    pending_action: Option<PendingAction>,
    status: String,
    busy: bool,
    last_poll: Option<std::time::Instant>,
    proxy: Option<exchange::proxy::Proxy>,
}

impl Trader {
    pub fn new(mut config: TradingConfig, proxy: Option<exchange::proxy::Proxy>) -> Self {
        config.poll_interval_secs = config.poll_interval_secs.clamp(1, 60);
        config.recv_window_ms = config.recv_window_ms.clamp(1_000, 60_000);
        let credentials = data::config::trading::load_credentials(config.testnet);
        Self {
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
            side: Side::Buy,
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
            pending_action: None,
            status: "Enter Binance USD-M API credentials and connect.".to_string(),
            busy: false,
            last_poll: None,
            proxy,
        }
    }

    pub fn config(&self) -> &TradingConfig {
        &self.config
    }

    pub fn has_credentials(&self) -> bool {
        !self.api_key.is_empty() && !self.secret_key.is_empty()
    }

    pub fn set_chart_sources(&mut self, sources: Vec<ChartSource>) {
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
    }

    pub fn selected_chart(&self) -> Option<uuid::Uuid> {
        self.selected_chart
    }

    pub fn overlay(&self) -> Option<PositionOverlay> {
        let snapshot = self.snapshot.as_ref()?;
        let position = snapshot
            .positions
            .iter()
            .find(|position| position.symbol == self.config.symbol)?;
        let stop_loss = snapshot
            .open_orders
            .iter()
            .find(|order| {
                order.symbol == position.symbol
                    && order.order_type.contains("STOP")
                    && !order.order_type.contains("TAKE_PROFIT")
            })
            .map(|order| order.stop_price)
            .filter(|price| *price > 0.0);
        let take_profit = snapshot
            .open_orders
            .iter()
            .find(|order| {
                order.symbol == position.symbol && order.order_type.contains("TAKE_PROFIT")
            })
            .map(|order| order.stop_price)
            .filter(|price| *price > 0.0);
        Some(PositionOverlay {
            symbol: position.symbol.clone(),
            is_long: position.is_long(),
            entry_price: position.entry_price,
            break_even_price: position.economic_break_even_price,
            liquidation_price: (position.liquidation_price > 0.0)
                .then_some(position.liquidation_price),
            stop_loss,
            take_profit,
            unrealized_pnl: position.unrealized_pnl,
        })
    }

    pub fn tick(&mut self, now: std::time::Instant) -> Task<Message> {
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
            Message::ApiKeyChanged(value) => self.api_key = value,
            Message::SecretKeyChanged(value) => self.secret_key = value,
            Message::ToggleTestnet(value) => {
                self.config.testnet = value;
                self.client = None;
                self.snapshot = None;
                let credentials = data::config::trading::load_credentials(value);
                self.api_key = credentials
                    .as_ref()
                    .map(|value| value.api_key.clone())
                    .unwrap_or_default();
                self.secret_key = credentials
                    .as_ref()
                    .map(|value| value.secret_key.clone())
                    .unwrap_or_default();
                self.status = if value {
                    "Binance USD-M testnet selected.".to_string()
                } else {
                    "Binance USD-M live trading selected. Orders use real funds.".to_string()
                };
            }
            Message::ConfirmOrdersChanged(value) => self.config.confirm_orders = value,
            Message::SaveCredentials => {
                self.status = match data::config::trading::save_credentials(
                    self.config.testnet,
                    &self.api_key,
                    &self.secret_key,
                ) {
                    Ok(()) => "Trading credentials saved to the system keychain.".to_string(),
                    Err(error) => error,
                };
            }
            Message::Connect => {
                if self.busy {
                    return Task::none();
                }
                let credentials = TradingCredentials {
                    api_key: self.api_key.trim().to_string(),
                    secret_key: self.secret_key.trim().to_string(),
                };
                if credentials.api_key.is_empty() || credentials.secret_key.is_empty() {
                    self.status = "API key and secret key are required.".to_string();
                    return Task::none();
                }
                if let Err(error) = data::config::trading::save_credentials(
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
                self.status = "Connecting to Binance...".to_string();
                return Task::perform(
                    async move { client.connect(&symbol).await },
                    Message::Connected,
                );
            }
            Message::Connected(result) => {
                self.busy = false;
                match result {
                    Ok((client, snapshot)) => {
                        self.client = Some(client);
                        self.snapshot = Some(snapshot);
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
                        self.status = error;
                    }
                }
            }
            Message::Disconnect => {
                self.client = None;
                self.snapshot = None;
                self.pending_action = None;
                self.status = "Disconnected from Binance.".to_string();
            }
            refresh @ (Message::Refresh | Message::Poll) => {
                let Some(client) = self.client.clone() else {
                    return Task::none();
                };
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
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
                match result {
                    Ok(mut snapshot) => {
                        if let Some(previous) = &self.snapshot {
                            preserve_position_costs(previous, &mut snapshot);
                        }
                        self.snapshot = Some(snapshot);
                        self.last_poll = Some(std::time::Instant::now());
                        self.status = "Account synchronized.".to_string();
                    }
                    Err(error) => self.status = error,
                }
            }
            Message::SymbolChanged(value) => {
                self.config.symbol = normalize_symbol(&value);
            }
            Message::SideChanged(value) => self.side = value,
            Message::OrderTypeChanged(value) => self.order_type = value,
            Message::MarginModeChanged(value) => self.margin_mode = value,
            Message::QuantityChanged(value) => self.quantity = numeric_input(value),
            Message::PriceChanged(value) => self.price = numeric_input(value),
            Message::StopLossChanged(value) => self.stop_loss = numeric_input(value),
            Message::TakeProfitChanged(value) => self.take_profit = numeric_input(value),
            Message::LeverageChanged(value) => self.leverage = integer_input(value),
            Message::RiskPercentChanged(value) => self.risk_percent = numeric_input(value),
            Message::RewardRatioChanged(value) => self.reward_ratio = numeric_input(value),
            Message::CalculateRiskLevels => self.calculate_risk_levels(),
            Message::ReduceOnlyChanged(value) => self.reduce_only = value,
            Message::ChartSourceChanged(source) => {
                self.selected_chart = Some(source.pane_id);
                self.config.symbol = source.symbol;
                if self.client.is_some() {
                    return Task::done(Message::Refresh);
                }
            }
            Message::PlaceOrder => {
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
                    self.status = "Leverage must be between 1 and 125.".to_string();
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
            Message::ApplyProtection(index) => {
                let Some(position) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.positions.get(index))
                    .cloned()
                else {
                    return Task::none();
                };
                let stop_loss = optional_price(&self.stop_loss);
                let take_profit = optional_price(&self.take_profit);
                if stop_loss.is_none() && take_profit.is_none() {
                    self.status = "Enter a stop loss or take profit price first.".to_string();
                    return Task::none();
                }
                return self.queue_or_execute(PendingAction::Protect {
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
            Message::CancelAction => self.pending_action = None,
            Message::ActionCompleted(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(message) => message,
                    Err(error) => error,
                };
                if self.client.is_some() {
                    return Task::done(Message::Refresh);
                }
            }
        }
        Task::none()
    }

    fn order_request(&self) -> Result<OrderRequest, String> {
        if self.reduce_only
            && (optional_price(&self.stop_loss).is_some()
                || optional_price(&self.take_profit).is_some())
        {
            return Err(
                "Attached TP/SL is not available on a reduce-only order. Use the position TP/SL action instead."
                    .to_string(),
            );
        }
        let quantity = parse_input(&self.quantity, "Quantity")?;
        let price = if self.order_type == OrderType::Limit {
            Some(parse_input(&self.price, "Limit price")?)
        } else {
            None
        };
        let position_side = if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.hedge_mode)
        {
            match self.side {
                Side::Buy => PositionSide::Long,
                Side::Sell => PositionSide::Short,
            }
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
            stop_loss: optional_price(&self.stop_loss),
            take_profit: optional_price(&self.take_profit),
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
        if self.client.is_none() {
            self.status = "Connect a Binance account first.".to_string();
            return Task::none();
        }
        if self.config.confirm_orders || !self.config.testnet {
            self.pending_action = Some(action);
            Task::none()
        } else {
            self.execute(action)
        }
    }

    fn execute(&mut self, action: PendingAction) -> Task<Message> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        self.busy = true;
        self.status = "Sending trading request...".to_string();
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
                        Ok("Position protection orders accepted.".to_string())
                    }
                }
            },
            Message::ActionCompleted,
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        let connected = self.client.is_some();
        let selected_chart = self
            .selected_chart
            .and_then(|id| {
                self.chart_sources
                    .iter()
                    .find(|source| source.pane_id == id)
            })
            .cloned();
        let account_header: Element<'_, Message> = self.snapshot.as_ref().map_or_else(
            || row![text("No account data")].into(),
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

        let credentials = column![
            row![
                checkbox(self.config.testnet)
                    .label("Testnet")
                    .on_toggle(Message::ToggleTestnet),
                text(if self.config.testnet {
                    "SIMULATED"
                } else {
                    "LIVE FUNDS"
                }),
                checkbox(self.config.confirm_orders)
                    .label("Confirm orders")
                    .on_toggle(Message::ConfirmOrdersChanged),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                text_input("API key", &self.api_key)
                    .secure(true)
                    .on_input(Message::ApiKeyChanged)
                    .width(Length::Fill),
                text_input("Secret key", &self.secret_key)
                    .secure(true)
                    .on_input(Message::SecretKeyChanged)
                    .width(Length::Fill),
            ]
            .spacing(6),
            row![
                button(text("Save")).on_press(Message::SaveCredentials),
                if connected {
                    button(text("Disconnect")).on_press(Message::Disconnect)
                } else if self.busy {
                    button(text("Connecting"))
                } else {
                    button(text("Connect")).on_press(Message::Connect)
                },
            ]
            .spacing(6),
        ]
        .spacing(6);

        let order_ticket = column![
            row![
                text_input("Symbol", &self.config.symbol)
                    .on_input(Message::SymbolChanged)
                    .width(Length::Fill),
                pick_list(Side::ALL, Some(self.side), Message::SideChanged).width(Length::Fill),
            ]
            .spacing(6),
            row![
                pick_list(
                    OrderType::ALL,
                    Some(self.order_type),
                    Message::OrderTypeChanged
                )
                .width(Length::Fill),
                pick_list(
                    MarginMode::ALL,
                    Some(self.margin_mode),
                    Message::MarginModeChanged
                )
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                text_input("Quantity", &self.quantity)
                    .on_input(Message::QuantityChanged)
                    .width(Length::Fill),
                text_input("Limit price", &self.price)
                    .on_input(Message::PriceChanged)
                    .width(Length::Fill),
            ]
            .spacing(6),
            row![
                text_input("Stop loss", &self.stop_loss)
                    .on_input(Message::StopLossChanged)
                    .width(Length::Fill),
                text_input("Take profit", &self.take_profit)
                    .on_input(Message::TakeProfitChanged)
                    .width(Length::Fill),
            ]
            .spacing(6),
            row![
                text_input("Leverage 1-125", &self.leverage)
                    .on_input(Message::LeverageChanged)
                    .width(Length::Fill),
                checkbox(self.reduce_only)
                    .label("Reduce only")
                    .on_toggle(Message::ReduceOnlyChanged),
                if self.busy {
                    button(text("Working"))
                } else {
                    button(text("Place order")).on_press(Message::PlaceOrder)
                },
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                text_input("Risk %", &self.risk_percent)
                    .on_input(Message::RiskPercentChanged)
                    .width(Length::Fill),
                text_input("Reward ratio", &self.reward_ratio)
                    .on_input(Message::RewardRatioChanged)
                    .width(Length::Fill),
            ]
            .spacing(6),
            row![
                button(text("Calculate TP/SL")).on_press(Message::CalculateRiskLevels),
                text(self.risk_preview())
                    .size(crate::style::text_size::SMALL)
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                text("Chart"),
                pick_list(
                    self.chart_sources.clone(),
                    selected_chart,
                    Message::ChartSourceChanged
                )
                .placeholder("No compatible chart")
                .width(Length::Fill),
                button(text("Refresh")).on_press(Message::Refresh),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(6);

        let positions = self.positions_view();
        let orders = self.orders_view();
        let confirmation: Element<'_, Message> = if let Some(action) = &self.pending_action {
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
                } => format!(
                    "Confirm protection for {} | SL {} | TP {}",
                    position.symbol,
                    stop_loss
                        .map(format_decimal)
                        .unwrap_or_else(|| "--".to_string()),
                    take_profit
                        .map(format_decimal)
                        .unwrap_or_else(|| "--".to_string())
                ),
            };
            container(
                row![
                    text(summary).width(Length::Fill),
                    button(text("Cancel")).on_press(Message::CancelAction),
                    button(text(if self.config.testnet {
                        "Confirm"
                    } else {
                        "Confirm live order"
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

        container(scrollable(
            column![
                credentials,
                rule::horizontal(1),
                account_header,
                order_ticket,
                confirmation,
                text(&self.status).size(crate::style::text_size::SMALL),
                rule::horizontal(1),
                positions,
                rule::horizontal(1),
                orders,
            ]
            .spacing(10)
            .padding(padding::all(12)),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn positions_view(&self) -> Element<'_, Message> {
        let mut list = column![text("Positions").size(crate::style::text_size::SECTION)].spacing(4);
        let Some(snapshot) = &self.snapshot else {
            return list.push(text("No positions")).into();
        };
        for (index, position) in snapshot.positions.iter().enumerate() {
            let positive = position.unrealized_pnl >= 0.0;
            list = list.push(
                row![
                    text(format!(
                        "{} {} {:.6} | mark {:.4} | entry {:.4} | BE {:.4} | liq {:.4} | funding {:.4} | {}x {}",
                        position.symbol,
                        if position.is_long() { "LONG" } else { "SHORT" },
                        position.quantity.abs(),
                        position.mark_price,
                        position.entry_price,
                        position.economic_break_even_price,
                        position.liquidation_price,
                        position.funding,
                        position.leverage,
                        position.margin_mode,
                    ))
                    .width(Length::Fill),
                    text(format!("PnL {:.2}", position.unrealized_pnl)).style(
                        move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(if positive {
                                theme.extended_palette().success.base.color
                            } else {
                                theme.extended_palette().danger.base.color
                            }),
                        }
                    ),
                    button(text("TP/SL")).on_press(Message::ApplyProtection(index)),
                    button(text("Close")).on_press(Message::ClosePosition(index)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if snapshot.positions.is_empty() {
            list = list.push(text("No open positions"));
        }
        list.into()
    }

    fn orders_view(&self) -> Element<'_, Message> {
        let mut list = column![
            row![
                text("Open orders")
                    .size(crate::style::text_size::SECTION)
                    .width(Length::Fill),
                button(text("Cancel all")).on_press(Message::CancelAllOrders),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(4);
        let Some(snapshot) = &self.snapshot else {
            return list.push(text("No orders")).into();
        };
        for order in &snapshot.open_orders {
            list = list.push(
                row![
                    text(format!(
                        "{} {} {} {:.6} | price {:.4} | stop {:.4}",
                        order.symbol,
                        order.side,
                        order.order_type,
                        order.quantity,
                        order.price,
                        order.stop_price
                    ))
                    .width(Length::Fill),
                    button(text("Cancel")).on_press(Message::CancelOrder(
                        order.symbol.clone(),
                        order.order_id,
                        order.is_algo,
                    )),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if snapshot.open_orders.is_empty() {
            list = list.push(text("No open orders"));
        }
        list.into()
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
            self.status =
                "Risk %, reward ratio and a position or limit price are required.".to_string();
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
        self.stop_loss = format_decimal(stop_loss);
        self.take_profit = format_decimal(take_profit);
        self.status =
            "TP/SL levels calculated from net break-even, including entry/exit fees and funding."
                .to_string();
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
            .map(|price| format!("SL PnL {:.2}", position.estimated_pnl_at(price, fee)));
        let tp = optional_price(&self.take_profit)
            .map(|price| format!("TP PnL {:.2}", position.estimated_pnl_at(price, fee)));
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

fn integer_input(value: String) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect()
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
        let mut positions = Vec::new();
        for raw in raw_positions {
            let quantity = parse_number(&raw.position_amt)?;
            if quantity.abs() <= f64::EPSILON {
                continue;
            }
            let side = parse_position_side(&raw.position_side)?;
            let costs = if include_costs {
                let trades = self.user_trades(&raw.symbol).await?;
                current_position_costs(quantity, side, &trades)
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

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "Binance request timed out.".to_string()
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
    qty: String,
    side: String,
    position_side: String,
    time: u64,
    #[serde(default)]
    commission: String,
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
    use super::*;

    fn trade(time: u64, side: &str, qty: &str) -> RawUserTrade {
        RawUserTrade {
            qty: qty.to_string(),
            side: side.to_string(),
            position_side: "BOTH".to_string(),
            time,
            commission: "0.1".to_string(),
            realized_pnl: "0".to_string(),
        }
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
    fn fee_aware_levels_preserve_net_risk_reward() {
        let (stop, target) = net_risk_levels(100.0, 1.0, 2.0, 0.001, true);
        let net = |price: f64| price - 100.0 - price * 0.001;
        assert!((net(stop) + 1.0).abs() < 1e-9);
        assert!((net(target) - 2.0).abs() < 1e-9);
    }
}
