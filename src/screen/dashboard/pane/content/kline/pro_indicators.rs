use super::Message;
use data::chart::kline::pro::{OrderFlowBiasConfig, OrderFlowBiasPreset, ProConfig, QuantityRounding, VolumeProfileDisplay, VolumeProfilePastEnd, VolumeProfileRange, VwapMode};
use iced::{Element, Length, widget::{checkbox, column, pick_list, row, slider, text, text_input}};

fn number(config: &ProConfig, label: &'static str, value: f32, min: f32, max: f32, step: f32, update: fn(&mut ProConfig, f32)) -> Element<'static, Message> {
    let config = config.clone();
    column![text(format!("{label}: {value}")), slider(min..=max, value, move |value| {
        let mut config = config.clone(); update(&mut config,value); Message::ProConfigChanged(config)
    }).step(step)].spacing(4).into()
}

fn toggle(config: &ProConfig, name: &'static str, label: &'static str) -> Element<'static, Message> {
    let selected = config.is_enabled(name);
    let config = config.clone();
    checkbox(selected).label(label).on_toggle(move |enabled| {
        let mut config = config.clone();
        config.enabled.retain(|entry| entry != name);
        if enabled { config.enabled.push(name.into()); }
        Message::ProConfigChanged(config)
    }).into()
}

pub(super) fn view(config: &ProConfig) -> Element<'static, Message> {
    let summary = config.clone();
    let orderbook = config.clone();
    let rounding = config.clone();
    let preset = config.clone();
    let profile_display = config.clone();
    let profile_range = config.clone();
    let profile_end = config.clone();
    let mode_config = config.clone();
    let anchor = config.clone();
    let modes = ["Session", "Rolling", "Week", "Month", "Visible"];
    let current_mode = match config.vwap_mode { Some(VwapMode::Rolling{..}) => "Rolling", Some(VwapMode::Week) => "Week", Some(VwapMode::Month) => "Month", Some(VwapMode::Visible{..}) => "Visible", _ => "Session" };
    let mut controls = column![
        text("Pro").size(14),
        checkbox(config.show_footprint_summary).label("Footprint summary").on_toggle(move |value| { let mut c=summary.clone();c.show_footprint_summary=value;Message::ProConfigChanged(c) }),
        checkbox(config.show_footprint_orderbook).label("Footprint order book").on_toggle(move |value| { let mut c=orderbook.clone();c.show_footprint_orderbook=value;Message::ProConfigChanged(c) }),
        number(config,"Quantity decimals",f32::from(config.quantity_format.decimals),0.0,6.0,1.0,|c,v|c.quantity_format.decimals=v as u8),
        pick_list(QuantityRounding::ALL,Some(config.quantity_format.rounding),move |value| { let mut c=rounding.clone();c.quantity_format.rounding=value;Message::ProConfigChanged(c) }),
        toggle(config,"SessionVwap","VWAP"),
        pick_list(modes,Some(current_mode),move |value| { let mut c=mode_config.clone(); c.vwap_mode=match value { "Rolling"=>Some(VwapMode::Rolling{bars:20}),"Week"=>Some(VwapMode::Week),"Month"=>Some(VwapMode::Month),"Visible"=>Some(VwapMode::Visible{time_ms:0}),_=>None };Message::ProConfigChanged(c) }),
        number(config,"Session anchor (UTC hour)",f32::from(config.session_vwap.anchor_hour_utc),0.0,23.0,1.0,|c,v|c.session_vwap.anchor_hour_utc=v as u8),
        number(config,"VWAP bands",f32::from(config.session_vwap.deviation_bands),0.0,6.0,1.0,|c,v|c.session_vwap.deviation_bands=v as u8),
        number(config,"VWAP width",config.session_vwap.line_width,0.5,5.0,0.25,|c,v|c.session_vwap.line_width=v),
        toggle(config,"AnchoredVwap","Anchored VWAP"),
        text_input("Anchor Unix milliseconds",&config.anchored_vwap.anchor.map(|a| a.as_u64().to_string()).unwrap_or_default()).on_input(move |value| { let mut c=anchor.clone(); if value.is_empty(){c.anchored_vwap.anchor=None;} else if let Ok(v)=value.parse(){c.anchored_vwap.anchor=Some(exchange::UnixMs::new(v));} Message::ProConfigChanged(c) }),
        number(config,"Anchored VWAP bands",f32::from(config.anchored_vwap.deviation_bands),0.0,6.0,1.0,|c,v|c.anchored_vwap.deviation_bands=v as u8),
        number(config,"Anchored VWAP width",config.anchored_vwap.line_width,0.5,5.0,0.25,|c,v|c.anchored_vwap.line_width=v),
        toggle(config,"OrderFlowBias","Order Flow Bias"),
        pick_list(OrderFlowBiasPreset::ALL,Some(config.order_flow_bias.preset),move |value| { let mut c=preset.clone();c.order_flow_bias=if value==OrderFlowBiasPreset::Custom{c.order_flow_bias.custom()}else{OrderFlowBiasConfig::from_preset(value)};Message::ProConfigChanged(c) }),
        number(config,"Bias lookback",f32::from(config.order_flow_bias.lookback),2.0,100.0,1.0,|c,v|{c.order_flow_bias.lookback=v as u16;c.order_flow_bias=c.order_flow_bias.custom();}),
        number(config,"Signal threshold",config.order_flow_bias.signal_threshold,5.0,100.0,1.0,|c,v|{c.order_flow_bias.signal_threshold=v;c.order_flow_bias=c.order_flow_bias.custom();}),
        number(config,"Absorption delta",config.order_flow_bias.absorption_delta,0.1,0.95,0.05,|c,v|{c.order_flow_bias.absorption_delta=v;c.order_flow_bias=c.order_flow_bias.custom();}),
        number(config,"Max price response",config.order_flow_bias.max_price_response,0.05,1.0,0.05,|c,v|{c.order_flow_bias.max_price_response=v;c.order_flow_bias=c.order_flow_bias.custom();}),
        number(config,"Imbalance ratio",config.order_flow_bias.imbalance_ratio,1.1,10.0,0.1,|c,v|{c.order_flow_bias.imbalance_ratio=v;c.order_flow_bias=c.order_flow_bias.custom();}),
        toggle(config,"LiquidationHeatmap","Liquidation heatmap"),
        number(config,"Liquidation row height",config.liquidation_heatmap.row_height,1.0,30.0,1.0,|c,v|c.liquidation_heatmap.row_height=v),
        number(config,"Liquidation opacity",config.liquidation_heatmap.opacity,0.05,1.0,0.05,|c,v|c.liquidation_heatmap.opacity=v),
        toggle(config,"VolumeProfile","Left volume profile"),
        toggle(config,"FixedVolumeProfile","Fixed window volume profile"),
        row![pick_list(VolumeProfileDisplay::ALL,Some(config.volume_profile.display),move |value|{let mut c=profile_display.clone();c.volume_profile.display=value;Message::ProConfigChanged(c)}),pick_list(VolumeProfileRange::ALL,Some(config.volume_profile.range),move |value|{let mut c=profile_range.clone();c.volume_profile.range=value;Message::ProConfigChanged(c)})].spacing(4),
        pick_list(VolumeProfilePastEnd::ALL,Some(config.volume_profile.past_duration_end),move |value|{let mut c=profile_end.clone();c.volume_profile.past_duration_end=value;Message::ProConfigChanged(c)}),
        number(config,"Profile rows",f32::from(config.volume_profile.rows),10.0,400.0,1.0,|c,v|c.volume_profile.rows=v as u16),
        number(config,"Profile width (%)",f32::from(config.volume_profile.max_width_percent),5.0,50.0,1.0,|c,v|c.volume_profile.max_width_percent=v as u8),
        number(config,"Profile duration multiplier",f32::from(config.volume_profile.timeframe_multiplier),1.0,100.0,1.0,|c,v|c.volume_profile.timeframe_multiplier=v as u8),
    ].spacing(8).width(Length::Fill);
    if let Some(VwapMode::Rolling{bars}) = config.vwap_mode {
        controls = controls.push(number(config,"Rolling VWAP bars",f32::from(bars),1.0,1000.0,1.0,|c,v|c.vwap_mode=Some(VwapMode::Rolling{bars:v as u16})));
    }
    controls.into()
}
