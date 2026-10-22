use super::Message;
use data::chart::kline::pro::{OrderFlowBiasConfig, OrderFlowBiasPreset, ProConfig, QuantityRounding, VolumeProfileDisplay, VolumeProfilePastEnd, VolumeProfileRange, VwapMode};
use iced::{Element, Length, widget::{column, pick_list, row, slider, text, text_input, checkbox}};

fn number(config: &ProConfig, label: &'static str, value: f32, min: f32, max: f32, step: f32, update: fn(&mut ProConfig, f32)) -> Element<'static, Message> {
    let config = config.clone();
    column![text(format!("{label}: {value}")), slider(min..=max, value, move |value| {
        let mut config = config.clone(); update(&mut config,value); Message::ProConfigChanged(config)
    }).step(step)].spacing(4).into()
}

pub(crate) const ITEMS: &[(&str, &str)] = &[
    ("SessionVwap", "Session VWAP Pro"),
    ("AnchoredVwap", "Anchored VWAP Pro"),
    ("LiquidationHeatmap", "Liquidation Heatmap Pro"),
    ("VolumeProfile", "Volume Profile Pro"),
    ("FixedVolumeProfile", "Fixed Volume Profile Pro"),
    ("OrderBook", "Order Book Pro"),
];

pub(crate) fn footprint_view(config: &ProConfig) -> Element<'static, Message> {
    let summary = config.clone();
    let rounding = config.clone();
    column![
        checkbox(config.show_footprint_summary).label("Footprint summary").on_toggle(move |value| { let mut c=summary.clone();c.show_footprint_summary=value;Message::ProConfigChanged(c) }),
        number(config,"Quantity decimals",f32::from(config.quantity_format.decimals),0.0,6.0,1.0,|c,v|c.quantity_format.decimals=v as u8),
        pick_list(QuantityRounding::ALL,Some(config.quantity_format.rounding),move |value| { let mut c=rounding.clone();c.quantity_format.rounding=value;Message::ProConfigChanged(c) }),
    ].spacing(8).width(Length::Fill).into()
}

pub(crate) fn inputs(name: &str, config: &ProConfig) -> Element<'static, Message> {
    let preset = config.clone();
    let profile_display = config.clone();
    let profile_range = config.clone();
    let profile_end = config.clone();
    let mode_config = config.clone();
    let anchor = config.clone();
    let modes = ["Session", "Week", "Month", "Visible"];
    let current_mode = match config.vwap_mode { Some(VwapMode::Week) => "Week", Some(VwapMode::Month) => "Month", Some(VwapMode::Visible{..}) => "Visible", _ => "Session" };
    let mut controls = column![].spacing(8).width(Length::Fill);
    match name {
        "SessionVwap" => {
            controls = controls.push(pick_list(modes, Some(current_mode), move |value| { let mut c = mode_config.clone(); c.vwap_mode = match value { "Week" => Some(VwapMode::Week), "Month" => Some(VwapMode::Month), "Visible" => Some(VwapMode::Visible { time_ms: 0 }), _ => None }; Message::ProConfigChanged(c) }));
            controls = controls.push(number(config, "Session anchor (UTC hour)", f32::from(config.session_vwap.anchor_hour_utc), 0.0, 23.0, 1.0, |c, v| c.session_vwap.anchor_hour_utc = v as u8));
            controls = controls.push(number(config, "VWAP bands", f32::from(config.session_vwap.deviation_bands), 0.0, 6.0, 1.0, |c, v| c.session_vwap.deviation_bands = v as u8));
        }
        "AnchoredVwap" => {
            controls = controls.push(text_input("Anchor Unix milliseconds", &config.anchored_vwap.anchor.map(|anchor| anchor.as_u64().to_string()).unwrap_or_default()).on_input(move |value| { let mut c = anchor.clone(); if value.is_empty() { c.anchored_vwap.anchor = None; } else if let Ok(parsed) = value.parse() { c.anchored_vwap.anchor = Some(exchange::UnixMs::new(parsed)); } Message::ProConfigChanged(c) }));
            controls = controls.push(number(config, "Bands", f32::from(config.anchored_vwap.deviation_bands), 0.0, 6.0, 1.0, |c, v| c.anchored_vwap.deviation_bands = v as u8));
        }
        "OrderFlowBias" => {
            controls = controls.push(pick_list(OrderFlowBiasPreset::ALL, Some(config.order_flow_bias.preset), move |value| { let mut c = preset.clone(); c.order_flow_bias = if value == OrderFlowBiasPreset::Custom { c.order_flow_bias.custom() } else { OrderFlowBiasConfig::from_preset(value) }; Message::ProConfigChanged(c) }));
            controls = controls.push(number(config, "Lookback", f32::from(config.order_flow_bias.lookback), 2.0, 100.0, 1.0, |c, v| { c.order_flow_bias.lookback = v as u16; c.order_flow_bias = c.order_flow_bias.custom(); }));
            controls = controls.push(number(config, "Signal threshold", config.order_flow_bias.signal_threshold, 5.0, 100.0, 1.0, |c, v| { c.order_flow_bias.signal_threshold = v; c.order_flow_bias = c.order_flow_bias.custom(); }));
        }
        "LiquidationHeatmap" => {}
        "VolumeProfile" | "FixedVolumeProfile" => {
            controls = controls.push(row![
                pick_list(VolumeProfileDisplay::ALL, Some(config.volume_profile.display), move |value| { let mut c = profile_display.clone(); c.volume_profile.display = value; Message::ProConfigChanged(c) }),
                pick_list(VolumeProfileRange::ALL, Some(config.volume_profile.range), move |value| { let mut c = profile_range.clone(); c.volume_profile.range = value; Message::ProConfigChanged(c) }),
            ].spacing(4));
            controls = controls.push(pick_list(VolumeProfilePastEnd::ALL, Some(config.volume_profile.past_duration_end), move |value| { let mut c = profile_end.clone(); c.volume_profile.past_duration_end = value; Message::ProConfigChanged(c) }));
            controls = controls.push(number(config, "Rows", f32::from(config.volume_profile.rows), 10.0, 400.0, 1.0, |c, v| c.volume_profile.rows = v as u16));
            if name == "FixedVolumeProfile" {
                controls = controls.push(number(config, "Window multiplier", f32::from(config.fixed_volume_profile.timeframe_multiplier), 1.0, 100.0, 1.0, |c, v| c.fixed_volume_profile.timeframe_multiplier = v as u8));
            }
        }
        _ => {}
    }
    controls.into()
}

pub(crate) fn style(name: &str, config: &ProConfig) -> Element<'static, Message> {
    let mut controls = column![].spacing(8).width(Length::Fill);
    match name {
        "SessionVwap" => {
            controls = controls.push(number(config, "VWAP width", config.session_vwap.line_width, 0.5, 5.0, 0.25, |c, v| c.session_vwap.line_width = v));
        }
        "AnchoredVwap" => {
            controls = controls.push(number(config, "Width", config.anchored_vwap.line_width, 0.5, 5.0, 0.25, |c, v| c.anchored_vwap.line_width = v));
        }
        "LiquidationHeatmap" => {
            controls = controls.push(number(config, "Row height", config.liquidation_heatmap.row_height, 1.0, 30.0, 1.0, |c, v| c.liquidation_heatmap.row_height = v));
            controls = controls.push(number(config, "Opacity", config.liquidation_heatmap.opacity, 0.05, 1.0, 0.05, |c, v| c.liquidation_heatmap.opacity = v));
        }
        "VolumeProfile" | "FixedVolumeProfile" => {
            controls = controls.push(number(config, "Width (%)", f32::from(config.volume_profile.max_width_percent), 5.0, 50.0, 1.0, |c, v| c.volume_profile.max_width_percent = v as u8));
        }
        _ => {}
    }
    controls.into()
}
