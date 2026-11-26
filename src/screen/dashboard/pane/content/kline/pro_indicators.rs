use super::Message;
use data::chart::kline::pro::{
    ProConfig, QuantityRounding, VolumeProfileDisplay, VolumeProfilePastEnd, VolumeProfileRange,
    VwapMode,
};
use data::i18n::tr;
use iced::{
    Element, Length,
    widget::{checkbox, column, pick_list, row, slider, text, text_input},
};

fn number(
    config: &ProConfig,
    label: &'static str,
    value: f32,
    min: f32,
    max: f32,
    step: f32,
    update: fn(&mut ProConfig, f32),
) -> Element<'static, Message> {
    let config = config.clone();
    column![
        text(format!("{label}: {value}")),
        slider(min..=max, value, move |value| {
            let mut config = config.clone();
            update(&mut config, value);
            Message::ProConfigChanged(config)
        })
        .step(step)
    ]
    .spacing(4)
    .into()
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
    let aggregate = config.clone();
    let basis = config.clone();
    column![
        checkbox(config.aggregate_venues)
            .label(tr(
                "Aggregate venues (pick sources with Agg in the ticker selector)"
            ))
            .on_toggle(move |value| {
                let mut c = aggregate.clone();
                c.aggregate_venues = value;
                Message::ProConfigChanged(c)
            }),
        checkbox(config.aggregate_basis_adjust)
            .label(tr("Correct cross-venue price spread"))
            .on_toggle(move |value| {
                let mut c = basis.clone();
                c.aggregate_basis_adjust = value;
                Message::ProConfigChanged(c)
            }),
        checkbox(config.show_footprint_summary)
            .label(tr("Footprint summary"))
            .on_toggle(move |value| {
                let mut c = summary.clone();
                c.show_footprint_summary = value;
                Message::ProConfigChanged(c)
            }),
        number(
            config,
            tr("Quantity decimals"),
            f32::from(config.quantity_format.decimals),
            0.0,
            6.0,
            1.0,
            |c, v| c.quantity_format.decimals = v as u8
        ),
        pick_list(
            QuantityRounding::ALL,
            Some(config.quantity_format.rounding),
            move |value| {
                let mut c = rounding.clone();
                c.quantity_format.rounding = value;
                Message::ProConfigChanged(c)
            }
        ),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}

pub(crate) fn inputs(name: &str, config: &ProConfig) -> Element<'static, Message> {
    let profile_display = config.clone();
    let profile_range = config.clone();
    let profile_end = config.clone();
    let mode_config = config.clone();
    let anchor = config.clone();
    let modes = [tr("Session"), tr("Week"), tr("Month"), tr("Visible")];
    let current_mode = match config.vwap_mode {
        Some(VwapMode::Week) => tr("Week"),
        Some(VwapMode::Month) => tr("Month"),
        Some(VwapMode::Visible { .. }) => tr("Visible"),
        _ => tr("Session"),
    };
    let mut controls = column![].spacing(8).width(Length::Fill);
    match name {
        "SessionVwap" => {
            controls = controls.push(pick_list(modes, Some(current_mode), move |value| {
                let mut c = mode_config.clone();
                // The options are translated labels, so match against those.
                c.vwap_mode = if value == tr("Week") {
                    Some(VwapMode::Week)
                } else if value == tr("Month") {
                    Some(VwapMode::Month)
                } else if value == tr("Visible") {
                    Some(VwapMode::Visible { time_ms: 0 })
                } else {
                    None
                };
                Message::ProConfigChanged(c)
            }));
            controls = controls.push(number(
                config,
                tr("Session anchor (UTC hour)"),
                f32::from(config.session_vwap.anchor_hour_utc),
                0.0,
                23.0,
                1.0,
                |c, v| c.session_vwap.anchor_hour_utc = v as u8,
            ));
            controls = controls.push(number(
                config,
                tr("VWAP bands"),
                f32::from(config.session_vwap.deviation_bands),
                0.0,
                6.0,
                1.0,
                |c, v| c.session_vwap.deviation_bands = v as u8,
            ));
        }
        "AnchoredVwap" => {
            controls = controls.push(
                text_input(
                    "Anchor Unix ms (empty = first visible bar)",
                    &config
                        .anchored_vwap
                        .anchor
                        .map(|anchor| anchor.as_u64().to_string())
                        .unwrap_or_default(),
                )
                .on_input(move |value| {
                    let mut c = anchor.clone();
                    if value.is_empty() {
                        c.anchored_vwap.anchor = None;
                    } else if let Ok(parsed) = value.parse() {
                        c.anchored_vwap.anchor = Some(exchange::UnixMs::new(parsed));
                    }
                    Message::ProConfigChanged(c)
                }),
            );
            controls = controls.push(number(
                config,
                tr("Bands"),
                f32::from(config.anchored_vwap.deviation_bands),
                0.0,
                6.0,
                1.0,
                |c, v| c.anchored_vwap.deviation_bands = v as u8,
            ));
        }
        "LiquidationHeatmap" => {}
        "FixedVolumeProfile" => {
            controls = controls.push(pick_list(
                VolumeProfileDisplay::ALL,
                Some(config.fixed_volume_profile.display),
                move |value| {
                    let mut c = profile_display.clone();
                    c.fixed_volume_profile.display = value;
                    Message::ProConfigChanged(c)
                },
            ));
            controls = controls.push(number(
                config,
                tr("Rows"),
                f32::from(config.fixed_volume_profile.rows),
                16.0,
                400.0,
                1.0,
                |c, v| c.fixed_volume_profile.rows = v as u16,
            ));
            controls = controls.push(number(
                config,
                tr("Window multiplier"),
                f32::from(config.fixed_volume_profile.timeframe_multiplier),
                1.0,
                100.0,
                1.0,
                |c, v| c.fixed_volume_profile.timeframe_multiplier = v as u16,
            ));
        }
        "VolumeProfile" => {
            controls = controls.push(
                row![
                    pick_list(
                        VolumeProfileDisplay::ALL,
                        Some(config.volume_profile.display),
                        move |value| {
                            let mut c = profile_display.clone();
                            c.volume_profile.display = value;
                            Message::ProConfigChanged(c)
                        }
                    ),
                    pick_list(
                        VolumeProfileRange::ALL,
                        Some(config.volume_profile.range),
                        move |value| {
                            let mut c = profile_range.clone();
                            c.volume_profile.range = value;
                            Message::ProConfigChanged(c)
                        }
                    ),
                ]
                .spacing(4),
            );
            controls = controls.push(pick_list(
                VolumeProfilePastEnd::ALL,
                Some(config.volume_profile.past_duration_end),
                move |value| {
                    let mut c = profile_end.clone();
                    c.volume_profile.past_duration_end = value;
                    Message::ProConfigChanged(c)
                },
            ));
            if config.volume_profile.range == VolumeProfileRange::PastDuration {
                controls = controls.push(number(
                    config,
                    tr("Past duration (bars)"),
                    f32::from(config.volume_profile.timeframe_multiplier),
                    1.0,
                    5000.0,
                    1.0,
                    |c, v| c.volume_profile.timeframe_multiplier = v as u16,
                ));
            }
            controls = controls.push(number(
                config,
                tr("Rows (candles; footprint uses one row per tick)"),
                f32::from(config.volume_profile.rows),
                16.0,
                400.0,
                1.0,
                |c, v| c.volume_profile.rows = v as u16,
            ));
        }
        _ => {}
    }
    controls.into()
}

pub(crate) fn style(name: &str, config: &ProConfig) -> Element<'static, Message> {
    let mut controls = column![].spacing(8).width(Length::Fill);
    match name {
        "SessionVwap" => {
            controls = controls.push(number(
                config,
                tr("VWAP width"),
                config.session_vwap.line_width,
                0.5,
                5.0,
                0.25,
                |c, v| c.session_vwap.line_width = v,
            ));
        }
        "AnchoredVwap" => {
            controls = controls.push(number(
                config,
                tr("Width"),
                config.anchored_vwap.line_width,
                0.5,
                5.0,
                0.25,
                |c, v| c.anchored_vwap.line_width = v,
            ));
        }
        "LiquidationHeatmap" => {
            controls = controls.push(number(
                config,
                tr("Row height"),
                config.liquidation_heatmap.row_height,
                1.0,
                30.0,
                1.0,
                |c, v| c.liquidation_heatmap.row_height = v,
            ));
            controls = controls.push(number(
                config,
                tr("Opacity"),
                config.liquidation_heatmap.opacity,
                0.05,
                1.0,
                0.05,
                |c, v| c.liquidation_heatmap.opacity = v,
            ));
        }
        "VolumeProfile" => {
            controls = controls.push(number(
                config,
                tr("Width (%)"),
                f32::from(config.volume_profile.max_width_percent),
                5.0,
                50.0,
                1.0,
                |c, v| c.volume_profile.max_width_percent = v as u8,
            ));
        }
        "FixedVolumeProfile" => {
            controls = controls.push(number(
                config,
                tr("Width (%)"),
                f32::from(config.fixed_volume_profile.max_width_percent),
                5.0,
                100.0,
                1.0,
                |c, v| c.fixed_volume_profile.max_width_percent = v as u8,
            ));
        }
        _ => {}
    }
    controls.into()
}
