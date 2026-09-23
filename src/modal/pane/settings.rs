use crate::chart::comparison::ComparisonChart;
use crate::screen::dashboard::pane::{Event, Message};
use crate::screen::dashboard::panel::timeandsales;
use crate::split_column;
use crate::widget::{classic_slider_row, color_picker::color_picker, labeled_slider};
use crate::{style, tooltip, widget::scrollable_content};

use data::chart::heatmap::HeatmapStudy;
use data::chart::kline::FootprintStudy;
use data::chart::{
    KlineChartKind,
    heatmap::{self, CoalesceKind},
    kline::ClusterKind,
};
use data::layout::pane::VisualConfig;
use data::panel::ladder;
use data::panel::timeandsales::{StackedBar, StackedBarRatio};
use data::util::format_with_commas;

use iced::widget::{checkbox, space, text_input};
use iced::{
    Alignment, Element, Length,
    widget::{
        button, column, container, pane_grid, pick_list, radio, row, slider, text,
        tooltip::Position as TooltipPosition,
    },
};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KlineColorPicker {
    SessionVwap,
    CvdLine,
    CvdUp,
    CvdDown,
    StochRsiK,
    StochRsiD,
    OiLine,
    OiUp,
    OiDown,
    AnchoredVwap,
    Ema(usize),
    OidBullish,
    OidBearish,
    OidCvd,
    OidOi,
    OiFlowLong,
    OiFlowShort,
    OiFlowBuyOutflow,
    OiFlowSellOutflow,
    OiFlowExtremeBand,
    OwdPositive,
    OwdNegative,
    LpiPositive,
    LpiNegative,
    LiquidationHeatmapLong,
    LiquidationHeatmapShort,
}

fn cfg_view_container<'a, T>(max_width: u32, content: T) -> Element<'a, Message>
where
    T: Into<Element<'a, Message>>,
{
    container(scrollable_content(content))
        .width(Length::Shrink)
        .padding(28)
        .max_width(max_width)
        .style(style::chart_modal)
        .into()
}

pub fn heatmap_cfg_view<'a>(
    cfg: heatmap::Config,
    pane: pane_grid::Pane,
    study_config: &'a study::Configurator<HeatmapStudy>,
    studies: &'a [HeatmapStudy],
    basis: data::chart::Basis,
) -> Element<'a, Message> {
    let trade_size_slider = {
        let filter = cfg.trade_size_filter;
        labeled_slider(
            "Trade",
            0.0..=50000.0,
            filter,
            move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_filter: value,
                        ..cfg
                    }),
                    false,
                )
            },
            |value| format!(">${}", format_with_commas(*value as f64)),
            Some(500.0),
        )
    };

    let order_size_slider = {
        let filter = cfg.order_size_filter;
        labeled_slider(
            "Order",
            0.0..=500_000.0,
            filter,
            move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        order_size_filter: value,
                        ..cfg
                    }),
                    false,
                )
            },
            |value| format!(">${}", format_with_commas(*value as f64)),
            Some(5000.0),
        )
    };

    let circle_scaling_slider = cfg.trade_size_scale.map(|radius_scale| {
        classic_slider_row(
            text("Circle radius scaling"),
            slider(10..=200, radius_scale, move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_scale: Some(value),
                        ..cfg
                    }),
                    false,
                )
            })
            .step(10)
            .into(),
            Some(text(format!("{}%", radius_scale)).size(crate::style::text_size::EMPHASIS)),
        )
    });

    let coalescer_cfg: Option<Element<_>> = if let Some(coalescing) = cfg.coalescing {
        let threshold_pct = coalescing.threshold();

        let coalescer_kinds = {
            let average = radio(
                "Average",
                CoalesceKind::Average(threshold_pct),
                Some(coalescing),
                move |value| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::Heatmap(heatmap::Config {
                            coalescing: Some(value),
                            ..cfg
                        }),
                        false,
                    )
                },
            )
            .spacing(4);

            let first = radio(
                "First",
                CoalesceKind::First(threshold_pct),
                Some(coalescing),
                move |value| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::Heatmap(heatmap::Config {
                            coalescing: Some(value),
                            ..cfg
                        }),
                        false,
                    )
                },
            )
            .spacing(4);

            let max = radio(
                "Max",
                CoalesceKind::Max(threshold_pct),
                Some(coalescing),
                move |value| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::Heatmap(heatmap::Config {
                            coalescing: Some(value),
                            ..cfg
                        }),
                        false,
                    )
                },
            )
            .spacing(4);

            row![
                text("Merge method: "),
                row![average, first, max].spacing(12)
            ]
            .spacing(12)
        };

        let threshold_slider = classic_slider_row(
            text("Size similarity"),
            slider(0.05..=0.8, threshold_pct, move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        coalescing: Some(coalescing.with_threshold(value)),
                        ..cfg
                    }),
                    false,
                )
            })
            .step(0.05)
            .into(),
            Some(
                text(format!("{:.0}%", threshold_pct * 100.0))
                    .size(crate::style::text_size::EMPHASIS),
            ),
        );

        Some(
            container(column![coalescer_kinds, threshold_slider].spacing(8))
                .style(style::modal_container)
                .padding(8)
                .into(),
        )
    } else {
        None
    };

    let size_filters_column = column![
        text("Size filters").size(crate::style::text_size::SECTION),
        column![trade_size_slider, order_size_slider].spacing(8),
    ]
    .spacing(8);

    let noise_filters_column = {
        let merge_checkbox = checkbox(cfg.coalescing.is_some())
            .label("Merge orders if sizes are similar")
            .on_toggle(move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        coalescing: if value {
                            Some(CoalesceKind::Average(0.15))
                        } else {
                            None
                        },
                        ..cfg
                    }),
                    false,
                )
            });

        let mut col = column![
            text("Noise filters").size(crate::style::text_size::SECTION),
            merge_checkbox
        ]
        .spacing(8);
        if let Some(c) = coalescer_cfg {
            col = col.push(c);
        }
        col
    };

    let trade_viz_column = {
        let dyn_checkbox = checkbox(cfg.trade_size_scale.is_some())
            .label("Dynamic circle radius")
            .on_toggle(move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_scale: if value { Some(100) } else { None },
                        ..cfg
                    }),
                    false,
                )
            });

        let mut col = column![
            text("Trade visualization").size(crate::style::text_size::SECTION),
            dyn_checkbox
        ]
        .spacing(8);
        if let Some(slider) = circle_scaling_slider {
            col = col.push(slider);
        }
        col
    };

    let study_cfg = study_config.view(studies, basis).map(move |msg| {
        Message::PaneEvent(
            pane,
            Event::StudyConfigurator(study::StudyMessage::Heatmap(msg)),
        )
    });

    let content = split_column![
        size_filters_column,
        noise_filters_column,
        trade_viz_column,
        column![text("Studies").size(crate::style::text_size::SECTION), study_cfg].spacing(8),
        row![
            space::horizontal(),
            sync_all_button(pane, VisualConfig::Heatmap(cfg))
        ]
        ; spacing = 12, align_x = Alignment::Start
    ];

    cfg_view_container(360, content)
}

pub fn heatmap_shader_cfg_view<'a>(
    cfg: heatmap::Config,
    pane: pane_grid::Pane,
    study_config: &'a study::Configurator<HeatmapStudy>,
    studies: &'a [HeatmapStudy],
    basis: data::chart::Basis,
) -> Element<'a, Message> {
    let trade_size_slider = {
        let filter = cfg.trade_size_filter;
        labeled_slider(
            "Trade",
            0.0..=50000.0,
            filter,
            move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_filter: value,
                        ..cfg
                    }),
                    false,
                )
            },
            |value| format!(">${}", format_with_commas(*value as f64)),
            Some(500.0),
        )
    };

    let order_size_slider = {
        let filter = cfg.order_size_filter;
        labeled_slider(
            "Order",
            0.0..=500_000.0,
            filter,
            move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        order_size_filter: value,
                        ..cfg
                    }),
                    false,
                )
            },
            |value| format!(">${}", format_with_commas(*value as f64)),
            Some(5000.0),
        )
    };

    let circle_scaling_slider = cfg.trade_size_scale.map(|radius_scale| {
        classic_slider_row(
            text("Circle radius scaling"),
            slider(10..=200, radius_scale, move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_scale: Some(value),
                        ..cfg
                    }),
                    false,
                )
            })
            .step(10)
            .into(),
            Some(text(format!("{}%", radius_scale)).size(crate::style::text_size::EMPHASIS)),
        )
    });

    let size_filters_column = column![
        text("Size filters").size(crate::style::text_size::SECTION),
        column![trade_size_slider, order_size_slider].spacing(8),
    ]
    .spacing(8);

    let trade_viz_column = {
        let dyn_checkbox = checkbox(cfg.trade_size_scale.is_some())
            .label("Dynamic circle radius")
            .on_toggle(move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Heatmap(heatmap::Config {
                        trade_size_scale: if value { Some(100) } else { None },
                        ..cfg
                    }),
                    false,
                )
            });

        let mut col = column![
            text("Trade visualization").size(crate::style::text_size::SECTION),
            dyn_checkbox
        ]
        .spacing(8);
        if let Some(slider) = circle_scaling_slider {
            col = col.push(slider);
        }
        col
    };

    let study_cfg = study_config.view(studies, basis).map(move |msg| {
        Message::PaneEvent(
            pane,
            Event::StudyConfigurator(study::StudyMessage::Heatmap(msg)),
        )
    });

    let content = split_column![
        size_filters_column,
        trade_viz_column,
        column![text("Studies").size(crate::style::text_size::SECTION), study_cfg].spacing(8),
        row![
            space::horizontal(),
            sync_all_button(pane, VisualConfig::Heatmap(cfg))
        ]
        ; spacing = 12, align_x = Alignment::Start
    ];

    cfg_view_container(360, content)
}

pub fn timesales_cfg_view<'a>(
    cfg: timeandsales::Config,
    pane: pane_grid::Pane,
) -> Element<'a, Message> {
    let trade_size_column = {
        let filter = cfg.trade_size_filter;
        let slider = labeled_slider(
            "Trade",
            0.0..=50000.0,
            filter,
            move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::TimeAndSales(timeandsales::Config {
                        trade_size_filter: value,
                        ..cfg
                    }),
                    false,
                )
            },
            |value| format!(">${}", format_with_commas(*value as f64)),
            Some(500.0),
        );

        column![
            text("Size filter").size(crate::style::text_size::SECTION),
            slider
        ]
        .spacing(8)
    };

    let retention_minutes = (cfg.trade_retention.as_secs_f32() / 60.0).max(1.0);
    let retention_slider = {
        let slider_ui = slider(1.0..=60.0, retention_minutes, move |new_minutes| {
            let mins = new_minutes.round().max(1.0) as u64;
            Message::VisualConfigChanged(
                pane,
                VisualConfig::TimeAndSales(timeandsales::Config {
                    trade_retention: Duration::from_secs(mins * 60),
                    ..cfg
                }),
                false,
            )
        })
        .step(1.0);

        classic_slider_row(
            text("Keep trades for"),
            slider_ui.into(),
            Some(
                text(format!("≈ {} min", retention_minutes.round() as u64))
                    .size(crate::style::text_size::EMPHASIS),
            ),
        )
    };

    let history_column = column![
        row![
            text("History").size(crate::style::text_size::SECTION),
            tooltip(
                button("i").style(style::button::info),
                Some("Affects the stacked bar, colors and how much you can scroll down"),
                TooltipPosition::Top,
            )
        ]
        .spacing(4)
        .align_y(Alignment::Center),
        retention_slider
    ]
    .spacing(8);

    let stacked_bar: Element<_> = {
        let is_shown = cfg.stacked_bar.is_some();

        let enable_checkbox = checkbox(is_shown).label("Show stacked bar").on_toggle({
            move |value| {
                let current_ratio = cfg.stacked_bar.map(|h| h.ratio()).unwrap_or_default();
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::TimeAndSales(timeandsales::Config {
                        stacked_bar: if value {
                            Some(StackedBar::Compact(current_ratio))
                        } else {
                            None
                        },
                        ..cfg
                    }),
                    false,
                )
            }
        });

        let controls: Option<Element<_>> = cfg.stacked_bar.map(|hist| {
            let ratio = hist.ratio();
            let is_compact = matches!(hist, StackedBar::Compact(_));

            let compact = radio("Compact", true, Some(is_compact), {
                move |_v| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::TimeAndSales(timeandsales::Config {
                            stacked_bar: Some(StackedBar::Compact(ratio)),
                            ..cfg
                        }),
                        false,
                    )
                }
            })
            .spacing(4);

            let full = radio("Full", false, Some(is_compact), {
                move |_v| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::TimeAndSales(timeandsales::Config {
                            stacked_bar: Some(StackedBar::Full(ratio)),
                            ..cfg
                        }),
                        false,
                    )
                }
            })
            .spacing(4);

            let metric_picklist = pick_list(StackedBarRatio::ALL, Some(ratio), move |new_ratio| {
                let new_hist = Some(match cfg.stacked_bar {
                    Some(StackedBar::Full(_)) => StackedBar::Full(new_ratio),
                    _ => StackedBar::Compact(new_ratio),
                });
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::TimeAndSales(timeandsales::Config {
                        stacked_bar: new_hist,
                        ..cfg
                    }),
                    false,
                )
            });

            column![
                iced::widget::rule::horizontal(1),
                text("Mode").size(crate::style::text_size::BODY),
                row![compact, full].spacing(12),
                text("Metric").size(crate::style::text_size::BODY),
                metric_picklist,
            ]
            .spacing(8)
            .into()
        });

        let mut inner = column![enable_checkbox]
            .width(Length::Fill)
            .padding(4)
            .spacing(8);

        if let Some(ctrls) = controls {
            inner = inner.push(ctrls);
        }

        container(inner)
            .style(style::modal_container)
            .padding(8)
            .into()
    };

    let content = split_column![
        trade_size_column,
        history_column,
        stacked_bar,
        row![space::horizontal(), sync_all_button(pane, VisualConfig::TimeAndSales(cfg))],
        ; spacing = 12, align_x = Alignment::Start
    ];

    cfg_view_container(320, content)
}

pub fn comparison_cfg_view<'a>(
    pane: pane_grid::Pane,
    chart: &'a ComparisonChart,
) -> Element<'a, Message> {
    let series = &chart.series;
    let series_editor = &chart.series_editor;

    let content = column![series_editor.view(series).map(move |msg| {
        Message::PaneEvent(
            pane,
            Event::ComparisonChartInteraction(crate::chart::comparison::Message::Editor(msg)),
        )
    })];

    cfg_view_container(320, content)
}

pub fn kline_cfg_view<'a>(
    study_config: &'a study::Configurator<FootprintStudy>,
    cfg: data::chart::kline::Config,
    kind: &'a KlineChartKind,
    pane: pane_grid::Pane,
    basis: data::chart::Basis,
) -> Element<'a, Message> {
    let display_readout_section = {
        let next_cfg = cfg.clone();
        let data_labels_checkbox = tooltip(
            checkbox(cfg.data_labels_always_visible)
                .label("Keep latest label visible")
                .on_toggle(move |value| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::Kline(data::chart::kline::Config {
                            data_labels_always_visible: value,
                            ..next_cfg.clone()
                        }),
                        false,
                    )
                }),
            Some("Show the latest datapoint label even when not hovering"),
            TooltipPosition::Top,
        );
        column![
            text("Data labels").size(crate::style::text_size::SECTION),
            data_labels_checkbox,
        ]
        .spacing(8)
    };

    let content = match kind {
        KlineChartKind::Candles => {
            split_column![
                display_readout_section,
                row![
                    space::horizontal(),
                    sync_all_button(pane, VisualConfig::Kline(cfg.clone()))
                ],
                ; spacing = 12, align_x = Alignment::Start
            ]
        }
        KlineChartKind::Footprint {
            clusters,
            scaling,
            studies,
        } => {
            let cluster_picklist =
                pick_list(ClusterKind::ALL, Some(clusters), move |new_cluster_kind| {
                    Message::PaneEvent(pane, Event::ClusterKindSelected(new_cluster_kind))
                });

            let footprint_cfg = cfg.clone();
            let footprint_summary_checkbox = tooltip(
                checkbox(cfg.show_footprint_summary)
                    .label("Show footprint summary")
                    .on_toggle(move |value| {
                        Message::VisualConfigChanged(
                            pane,
                            VisualConfig::Kline(data::chart::kline::Config {
                                show_footprint_summary: value,
                                ..footprint_cfg.clone()
                            }),
                            false,
                        )
                    }),
                Some("Show per-bar volume and delta below footprint candles"),
                TooltipPosition::Top,
            );

            let orderbook_cfg = cfg.clone();
            let orderbook_checkbox = checkbox(cfg.show_footprint_orderbook)
                .label("Show order book")
                .on_toggle(move |value| {
                    Message::VisualConfigChanged(
                        pane,
                        VisualConfig::Kline(data::chart::kline::Config {
                            show_footprint_orderbook: value,
                            ..orderbook_cfg.clone()
                        }),
                        false,
                    )
                });
            let decimals_cfg = cfg.clone();
            let quantity_decimals = slider(
                0..=6,
                cfg.quantity_format.decimals.min(6),
                move |decimals| {
                    let mut next = decimals_cfg.clone();
                    next.quantity_format.decimals = decimals;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let rounding_cfg = cfg.clone();
            let quantity_rounding = pick_list(
                data::chart::kline::QuantityRounding::ALL,
                Some(cfg.quantity_format.rounding),
                move |rounding| {
                    let mut next = rounding_cfg.clone();
                    next.quantity_format.rounding = rounding;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let scaling = {
                let picklist = pick_list(
                    data::chart::kline::ClusterScaling::ALL,
                    Some(scaling),
                    move |new_scaling| {
                        Message::PaneEvent(pane, Event::ClusterScalingSelected(new_scaling))
                    },
                );

                if let data::chart::kline::ClusterScaling::Hybrid { weight } = scaling {
                    let hybrid_slider = slider(0.0..=1.0, *weight, move |new_weight| {
                        Message::PaneEvent(
                            pane,
                            Event::ClusterScalingSelected(
                                data::chart::kline::ClusterScaling::Hybrid { weight: new_weight },
                            ),
                        )
                    })
                    .step(0.05);

                    column![
                        picklist,
                        hybrid_slider,
                        text("Blend visible-range and per-candle scaling"),
                    ]
                    .spacing(8)
                } else {
                    column![picklist].spacing(8)
                }
            };

            let available_studies: Vec<_> = data::chart::kline::FootprintStudy::ALL.to_vec();

            let active_studies: Vec<_> = studies
                .iter()
                .copied()
                .filter(|study| {
                    available_studies
                        .iter()
                        .any(|available| available.is_same_type(study))
                })
                .collect();

            let study_cfg = study_config
                .view_available(&active_studies, basis, available_studies)
                .map(move |msg| {
                    Message::PaneEvent(
                        pane,
                        Event::StudyConfigurator(study::StudyMessage::Footprint(msg)),
                    )
                });

            let mut content = split_column![
                display_readout_section,
                column![text("Footprint summary").size(crate::style::text_size::SECTION), footprint_summary_checkbox].spacing(8),
                column![
                    text("Order book").size(crate::style::text_size::SECTION),
                    orderbook_checkbox,
                ].spacing(8),
                column![
                    text("Quantity format").size(crate::style::text_size::SECTION),
                    row![
                        text(format!("Decimals: {}", cfg.quantity_format.decimals)),
                        quantity_decimals,
                    ].spacing(8).align_y(Alignment::Center),
                    row![text("Method"), quantity_rounding]
                        .spacing(8)
                        .align_y(Alignment::Center),
                ].spacing(8),
                column![text("Cluster type").size(crate::style::text_size::SECTION), cluster_picklist].spacing(8),
                ; spacing = 12, align_x = Alignment::Start
            ];

            content = content.push(
                column![
                    text("Cluster scaling").size(crate::style::text_size::SECTION),
                    scaling
                ]
                .spacing(8),
            );

            content = content
                .push(
                    column![
                        text("Studies").size(crate::style::text_size::SECTION),
                        study_cfg
                    ]
                    .spacing(8),
                )
                .push(row![
                    space::horizontal(),
                    sync_all_button(pane, VisualConfig::Kline(cfg.clone()))
                ]);

            content
        }
    };

    cfg_view_container(360, content)
}

pub fn kline_indicator_cfg_view<'a>(
    indicator: data::chart::indicator::KlineIndicator,
    cfg: data::chart::kline::Config,
    chart_kind: &data::chart::KlineChartKind,
    basis: data::chart::Basis,
    pane: pane_grid::Pane,
    expanded: Option<KlineColorPicker>,
) -> Element<'a, Message> {
    use data::chart::indicator::KlineIndicator;

    let content: Element<_> = match indicator {
        KlineIndicator::CumulativeDelta | KlineIndicator::OpenInterest => {
            let is_cvd = indicator == KlineIndicator::CumulativeDelta;
            let series = if is_cvd {
                cfg.cumulative_delta
            } else {
                cfg.open_interest
            };
            let style_cfg = cfg.clone();
            let style = pick_list(
                data::chart::kline::IndicatorPlotStyle::ALL,
                Some(series.style),
                move |style| {
                    let mut next = style_cfg.clone();
                    if is_cvd {
                        next.cumulative_delta.style = style;
                    } else {
                        next.open_interest.style = style;
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let width_cfg = cfg.clone();
            let width = slider(0.5..=5.0, series.line_width.clamp(0.5, 5.0), move |value| {
                let mut next = width_cfg.clone();
                if is_cvd {
                    next.cumulative_delta.line_width = value;
                } else {
                    next.open_interest.line_width = value;
                }
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.5);
            let line_cfg = cfg.clone();
            let line_picker = color_picker(
                data::config::theme::to_hsva(series.line_color),
                move |value| {
                    let mut next = line_cfg.clone();
                    if is_cvd {
                        next.cumulative_delta.line_color = data::config::theme::from_hsva(value);
                    } else {
                        next.open_interest.line_color = data::config::theme::from_hsva(value);
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let up_cfg = cfg.clone();
            let up_picker = color_picker(
                data::config::theme::to_hsva(series.up_color),
                move |value| {
                    let mut next = up_cfg.clone();
                    if is_cvd {
                        next.cumulative_delta.up_color = data::config::theme::from_hsva(value);
                    } else {
                        next.open_interest.up_color = data::config::theme::from_hsva(value);
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let down_cfg = cfg.clone();
            let down_picker = color_picker(
                data::config::theme::to_hsva(series.down_color),
                move |value| {
                    let mut next = down_cfg.clone();
                    if is_cvd {
                        next.cumulative_delta.down_color = data::config::theme::from_hsva(value);
                    } else {
                        next.open_interest.down_color = data::config::theme::from_hsva(value);
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let (line_swatch, up_swatch, down_swatch) = if is_cvd {
                (
                    KlineColorPicker::CvdLine,
                    KlineColorPicker::CvdUp,
                    KlineColorPicker::CvdDown,
                )
            } else {
                (
                    KlineColorPicker::OiLine,
                    KlineColorPicker::OiUp,
                    KlineColorPicker::OiDown,
                )
            };
            let colors: Element<_> = match series.style {
                data::chart::kline::IndicatorPlotStyle::Line => column![
                    text(format!("Line width: {:.1}", series.line_width)),
                    width,
                    row![
                        text("Line"),
                        color_swatch(pane, series.line_color, line_swatch)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    if expanded == Some(line_swatch) {
                        line_picker
                    } else {
                        space::vertical().height(0).into()
                    },
                ]
                .spacing(8)
                .into(),
                data::chart::kline::IndicatorPlotStyle::Candles => column![
                    row![
                        text("Up"),
                        color_swatch(pane, series.up_color, up_swatch),
                        text("Down"),
                        color_swatch(pane, series.down_color, down_swatch),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    if expanded == Some(up_swatch) {
                        up_picker
                    } else if expanded == Some(down_swatch) {
                        down_picker
                    } else {
                        space::vertical().height(0).into()
                    },
                ]
                .spacing(8)
                .into(),
            };
            column![
                text(if is_cvd { "CVD" } else { "Open Interest" })
                    .size(crate::style::text_size::SECTION),
                style,
                colors,
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::StochRsi => {
            let stoch = cfg.stoch_rsi;
            let rsi_cfg = cfg.clone();
            let rsi_length = slider(
                2.0..=50.0,
                stoch.rsi_length.clamp(2, 50) as f32,
                move |value| {
                    let mut next = rsi_cfg.clone();
                    next.stoch_rsi.rsi_length = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let stochastic_cfg = cfg.clone();
            let stochastic_length = slider(
                2.0..=50.0,
                stoch.stochastic_length.clamp(2, 50) as f32,
                move |value| {
                    let mut next = stochastic_cfg.clone();
                    next.stoch_rsi.stochastic_length = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let k_cfg = cfg.clone();
            let k_smoothing = slider(
                1.0..=10.0,
                stoch.k_smoothing.clamp(1, 10) as f32,
                move |value| {
                    let mut next = k_cfg.clone();
                    next.stoch_rsi.k_smoothing = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let d_cfg = cfg.clone();
            let d_smoothing = slider(
                1.0..=10.0,
                stoch.d_smoothing.clamp(1, 10) as f32,
                move |value| {
                    let mut next = d_cfg.clone();
                    next.stoch_rsi.d_smoothing = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let width_cfg = cfg.clone();
            let width = slider(0.5..=5.0, stoch.line_width.clamp(0.5, 5.0), move |value| {
                let mut next = width_cfg.clone();
                next.stoch_rsi.line_width = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.5);
            let k_color_cfg = cfg.clone();
            let k_picker =
                color_picker(data::config::theme::to_hsva(stoch.k_color), move |value| {
                    let mut next = k_color_cfg.clone();
                    next.stoch_rsi.k_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            let d_color_cfg = cfg.clone();
            let d_picker =
                color_picker(data::config::theme::to_hsva(stoch.d_color), move |value| {
                    let mut next = d_color_cfg.clone();
                    next.stoch_rsi.d_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            column![
                text("Stoch RSI").size(crate::style::text_size::SECTION),
                text(format!("RSI length: {}", stoch.rsi_length)),
                rsi_length,
                text(format!("Stochastic length: {}", stoch.stochastic_length)),
                stochastic_length,
                text(format!("K smoothing: {}", stoch.k_smoothing)),
                k_smoothing,
                text(format!("D smoothing: {}", stoch.d_smoothing)),
                d_smoothing,
                text(format!("Line width: {:.1}", stoch.line_width)),
                width,
                row![
                    text("%K"),
                    color_swatch(pane, stoch.k_color, KlineColorPicker::StochRsiK),
                    text("%D"),
                    color_swatch(pane, stoch.d_color, KlineColorPicker::StochRsiD),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::StochRsiK) {
                    k_picker
                } else if expanded == Some(KlineColorPicker::StochRsiD) {
                    d_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::SessionVwap => {
            let vwap = cfg.session_vwap;
            let anchor_cfg = cfg.clone();
            let anchor = slider(0.0..=23.0, vwap.anchor_hour_utc as f32, move |value| {
                let mut next = anchor_cfg.clone();
                next.session_vwap.anchor_hour_utc = value as u8;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(1.0);
            let bands_cfg = cfg.clone();
            let bands = slider(1.0..=3.0, vwap.deviation_bands as f32, move |value| {
                let mut next = bands_cfg.clone();
                next.session_vwap.deviation_bands = value as u8;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(1.0);
            let color_cfg = cfg.clone();
            let picker = color_picker(data::config::theme::to_hsva(vwap.color), move |value| {
                let mut next = color_cfg.clone();
                next.session_vwap.color = data::config::theme::from_hsva(value);
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            });
            let width_cfg = cfg.clone();
            let width = slider(0.5..=5.0, vwap.line_width.clamp(0.5, 5.0), move |value| {
                let mut next = width_cfg.clone();
                next.session_vwap.line_width = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.5);
            column![
                text("Session VWAP").size(crate::style::text_size::SECTION),
                text(format!("UTC anchor: {:02}:00", vwap.anchor_hour_utc)),
                anchor,
                text(format!("Deviation bands: {}", vwap.deviation_bands)),
                bands,
                text(format!("Line width: {:.1}", vwap.line_width)),
                width,
                color_swatch(pane, vwap.color, KlineColorPicker::SessionVwap),
                if expanded == Some(KlineColorPicker::SessionVwap) {
                    picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::AnchoredVwap => {
            let vwap = cfg.anchored_vwap;
            let bands_cfg = cfg.clone();
            let bands = slider(0.0..=3.0, vwap.deviation_bands as f32, move |value| {
                let mut next = bands_cfg.clone();
                next.anchored_vwap.deviation_bands = value as u8;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(1.0);
            let color_cfg = cfg.clone();
            let picker = color_picker(data::config::theme::to_hsva(vwap.color), move |value| {
                let mut next = color_cfg.clone();
                next.anchored_vwap.color = data::config::theme::from_hsva(value);
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            });
            let width_cfg = cfg.clone();
            let width = slider(0.5..=5.0, vwap.line_width.clamp(0.5, 5.0), move |value| {
                let mut next = width_cfg.clone();
                next.anchored_vwap.line_width = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.5);
            let anchor = vwap
                .anchor
                .and_then(|time| time.format_utc("%Y-%m-%d %H:%M"))
                .unwrap_or_else(|| "Not selected".into());
            column![
                text("Anchored VWAP").size(crate::style::text_size::SECTION),
                text(format!("Anchor: {anchor}")),
                button("Select anchor on chart").on_press(Message::PaneEvent(
                    pane,
                    Event::BeginKlineIntervalSelection(
                        crate::chart::kline::IntervalSelection::AnchoredVwap
                    ),
                )),
                text(format!("Deviation bands: {}", vwap.deviation_bands)),
                bands,
                text(format!("Line width: {:.1}", vwap.line_width)),
                width,
                color_swatch(pane, vwap.color, KlineColorPicker::AnchoredVwap),
                if expanded == Some(KlineColorPicker::AnchoredVwap) {
                    picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::Ema => {
            let mut lines = column![text("EMA").size(crate::style::text_size::SECTION)].spacing(8);
            for (index, ema) in cfg.ema.iter().copied().enumerate() {
                let period_cfg = cfg.clone();
                let value = ema.period.max(1).to_string();
                let period = text_input("Period", &value)
                    .on_input(move |value| {
                        let mut next = period_cfg.clone();
                        if let Ok(period) = value.parse::<u16>()
                            && period > 0
                        {
                            next.ema[index].period = period;
                        }
                        Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                    })
                    .width(80);
                let color_cfg = cfg.clone();
                let picker = color_picker(data::config::theme::to_hsva(ema.color), move |value| {
                    let mut next = color_cfg.clone();
                    next.ema[index].color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
                let width_cfg = cfg.clone();
                let width = slider(0.5..=5.0, ema.line_width.clamp(0.5, 5.0), move |value| {
                    let mut next = width_cfg.clone();
                    next.ema[index].line_width = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                })
                .step(0.5);
                let mut removed = cfg.clone();
                removed.ema.remove(index);
                lines = lines.push(
                    column![
                        row![
                            text(format!("EMA {}", ema.period.max(1))),
                            space::horizontal(),
                            button(style::icon_text(style::Icon::TrashBin, 11)).on_press(
                                Message::VisualConfigChanged(
                                    pane,
                                    VisualConfig::Kline(removed),
                                    false
                                )
                            )
                        ]
                        .align_y(Alignment::Center),
                        row![
                            period,
                            color_swatch(pane, ema.color, KlineColorPicker::Ema(index))
                        ]
                        .spacing(8),
                        text(format!("Line width: {:.1}", ema.line_width)),
                        width,
                        if expanded == Some(KlineColorPicker::Ema(index)) {
                            picker
                        } else {
                            space::vertical().height(0).into()
                        },
                    ]
                    .spacing(6),
                );
            }
            let mut added = cfg.clone();
            let next_index = added.ema.len();
            added
                .ema
                .push(data::chart::kline::EmaConfig::for_index(next_index));
            lines
                .push(button("Add EMA").on_press(Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Kline(added),
                    false,
                )))
                .into()
        }
        KlineIndicator::VolumeProfile | KlineIndicator::FixedVolumeProfile => {
            let is_fixed = indicator == KlineIndicator::FixedVolumeProfile;
            let is_footprint = matches!(chart_kind, data::chart::KlineChartKind::Footprint { .. });
            let profile = if is_fixed {
                cfg.fixed_volume_profile
            } else {
                cfg.volume_profile
            };
            let display_cfg = cfg.clone();
            let display = pick_list(
                data::chart::kline::VolumeProfileDisplay::ALL,
                Some(profile.display),
                move |display| {
                    let mut next = display_cfg.clone();
                    if is_fixed {
                        next.fixed_volume_profile.display = display;
                    } else {
                        next.volume_profile.display = display;
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let range_cfg = cfg.clone();
            let range: Element<_> = pick_list(
                data::chart::kline::VolumeProfileRange::ALL,
                Some(profile.range),
                move |range| {
                    let mut next = range_cfg.clone();
                    next.volume_profile.range = range;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .into();
            let past_end_cfg = cfg.clone();
            let past_end = pick_list(
                data::chart::kline::VolumeProfilePastEnd::ALL,
                Some(profile.past_duration_end),
                move |past_duration_end| {
                    let mut next = past_end_cfg.clone();
                    next.volume_profile.past_duration_end = past_duration_end;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let multiplier_cfg = cfg.clone();
            let multiplier = slider(
                1..=100,
                profile.timeframe_multiplier.clamp(1, 100),
                move |value| {
                    let mut next = multiplier_cfg.clone();
                    if is_fixed {
                        next.fixed_volume_profile.timeframe_multiplier = value;
                    } else {
                        next.volume_profile.timeframe_multiplier = value;
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let multiplier_label = match basis {
                data::chart::Basis::Time(timeframe) => format!(
                    "{}x ({}m)",
                    profile.timeframe_multiplier,
                    timeframe.to_milliseconds() * u64::from(profile.timeframe_multiplier) / 60_000
                ),
                data::chart::Basis::Tick(_) => format!("{}x", profile.timeframe_multiplier),
            };
            let rows_cfg = cfg.clone();
            let rows_value = profile.rows.clamp(16, 400).to_string();
            let rows: Element<_> = text_input("Rows", &rows_value)
                .on_input(move |value| {
                    let mut next = rows_cfg.clone();
                    if let Ok(rows) = value.parse::<u16>() {
                        if is_fixed {
                            next.fixed_volume_profile.rows = rows.clamp(16, 400);
                        } else {
                            next.volume_profile.rows = rows.clamp(16, 400);
                        }
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                })
                .width(80)
                .into();
            let width_cfg = cfg.clone();
            let max_width = slider(
                5..=50,
                profile.max_width_percent.clamp(5, 50),
                move |value| {
                    let mut next = width_cfg.clone();
                    if is_fixed {
                        next.fixed_volume_profile.max_width_percent = value;
                    } else {
                        next.volume_profile.max_width_percent = value;
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let mut content = column![
                text(indicator.to_string()).size(crate::style::text_size::SECTION),
                row![text("Display"), display]
                    .spacing(8)
                    .align_y(Alignment::Center),
                row![
                    text(format!("Maximum width: {}%", profile.max_width_percent)),
                    max_width
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8);
            if !is_fixed {
                content = content.push(
                    row![text("Range"), range]
                        .spacing(8)
                        .align_y(Alignment::Center),
                );
            }
            if is_fixed || profile.range == data::chart::kline::VolumeProfileRange::PastDuration {
                if !is_fixed {
                    content = content.push(
                        row![text("End at"), past_end]
                            .spacing(8)
                            .align_y(Alignment::Center),
                    );
                }
                content = content.push(
                    row![
                        text(if is_fixed { "Window" } else { "Past duration" }),
                        text(multiplier_label),
                        multiplier
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
            }
            if !is_footprint {
                content = content.push(
                    row![text("Rows"), rows]
                        .spacing(8)
                        .align_y(Alignment::Center),
                );
            }
            content.into()
        }
        KlineIndicator::OiDeltaDivergence => {
            let oid = cfg.oi_delta_divergence;
            let lookback_cfg = cfg.clone();
            let lookback = text_input("Lookback", &oid.lookback.max(1).to_string())
                .on_input(move |value| {
                    let mut next = lookback_cfg.clone();
                    if let Ok(value) = value.parse::<u16>() {
                        next.oi_delta_divergence.lookback = value.clamp(1, 200);
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                })
                .width(80);
            let rsi_cfg = cfg.clone();
            let rsi_period = slider(
                2.0..=50.0,
                oid.rsi_period.clamp(2, 50) as f32,
                move |value| {
                    let mut next = rsi_cfg.clone();
                    next.oi_delta_divergence.rsi_period = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let size_cfg = cfg.clone();
            let marker_size = slider(2.0..=10.0, oid.marker_size.clamp(2.0, 10.0), move |value| {
                let mut next = size_cfg.clone();
                next.oi_delta_divergence.marker_size = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            });
            let bull_cfg = cfg.clone();
            let bull_picker = color_picker(
                data::config::theme::to_hsva(oid.bullish_color),
                move |value| {
                    let mut next = bull_cfg.clone();
                    next.oi_delta_divergence.bullish_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let bear_cfg = cfg.clone();
            let bear_picker = color_picker(
                data::config::theme::to_hsva(oid.bearish_color),
                move |value| {
                    let mut next = bear_cfg.clone();
                    next.oi_delta_divergence.bearish_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let cvd_cfg = cfg.clone();
            let cvd_picker =
                color_picker(data::config::theme::to_hsva(oid.cvd_color), move |value| {
                    let mut next = cvd_cfg.clone();
                    next.oi_delta_divergence.cvd_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            let oi_cfg = cfg.clone();
            let oi_picker =
                color_picker(data::config::theme::to_hsva(oid.oi_color), move |value| {
                    let mut next = oi_cfg.clone();
                    next.oi_delta_divergence.oi_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            let width_cfg = cfg.clone();
            let line_width = slider(0.5..=5.0, oid.line_width.clamp(0.5, 5.0), move |value| {
                let mut next = width_cfg.clone();
                next.oi_delta_divergence.line_width = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.5);
            column![
                text("OI-Delta Divergence").size(crate::style::text_size::SECTION),
                row![text("Lookback bars"), lookback]
                    .spacing(8)
                    .align_y(Alignment::Center),
                text(format!("RSI period: {}", oid.rsi_period)),
                rsi_period,
                text(format!("Marker size: {:.0}", oid.marker_size)),
                marker_size,
                text(format!("RSI line width: {:.1}", oid.line_width)),
                line_width,
                row![
                    text("CVD RSI"),
                    color_swatch(pane, oid.cvd_color, KlineColorPicker::OidCvd),
                    text("OI RSI"),
                    color_swatch(pane, oid.oi_color, KlineColorPicker::OidOi),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text("Bullish"),
                    color_swatch(pane, oid.bullish_color, KlineColorPicker::OidBullish),
                    text("Bearish"),
                    color_swatch(pane, oid.bearish_color, KlineColorPicker::OidBearish),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::OidBullish) {
                    bull_picker
                } else if expanded == Some(KlineColorPicker::OidBearish) {
                    bear_picker
                } else if expanded == Some(KlineColorPicker::OidCvd) {
                    cvd_picker
                } else if expanded == Some(KlineColorPicker::OidOi) {
                    oi_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::OiFlowNet => {
            let flow = cfg.oi_flow_net;
            let basis_cfg = cfg.clone();
            let basis_length = slider(
                2.0..=50.0,
                flow.basis_length.clamp(2, 50) as f32,
                move |value| {
                    let mut next = basis_cfg.clone();
                    next.oi_flow_net.basis_length = value as u16;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let sensitivity_cfg = cfg.clone();
            let sensitivity = slider(
                1.0..=5.0,
                flow.burst_sensitivity.clamp(1, 5) as f32,
                move |value| {
                    let mut next = sensitivity_cfg.clone();
                    next.oi_flow_net.burst_sensitivity = value as u8;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);
            let static_cfg = cfg.clone();
            let static_color = checkbox(flow.static_color)
                .label("Static colors")
                .on_toggle(move |value| {
                    let mut next = static_cfg.clone();
                    next.oi_flow_net.static_color = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            let price_cfg = cfg.clone();
            let price_line = checkbox(flow.show_price_line)
                .label("Show relative price line")
                .on_toggle(move |value| {
                    let mut next = price_cfg.clone();
                    next.oi_flow_net.show_price_line = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                });
            let long_cfg = cfg.clone();
            let long_picker = color_picker(
                data::config::theme::to_hsva(flow.long_color),
                move |value| {
                    let mut next = long_cfg.clone();
                    next.oi_flow_net.long_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let short_cfg = cfg.clone();
            let short_picker = color_picker(
                data::config::theme::to_hsva(flow.short_color),
                move |value| {
                    let mut next = short_cfg.clone();
                    next.oi_flow_net.short_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let buy_outflow_cfg = cfg.clone();
            let buy_outflow_picker = color_picker(
                data::config::theme::to_hsva(flow.buy_outflow_color),
                move |value| {
                    let mut next = buy_outflow_cfg.clone();
                    next.oi_flow_net.buy_outflow_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let sell_outflow_cfg = cfg.clone();
            let sell_outflow_picker = color_picker(
                data::config::theme::to_hsva(flow.sell_outflow_color),
                move |value| {
                    let mut next = sell_outflow_cfg.clone();
                    next.oi_flow_net.sell_outflow_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let extreme_band_cfg = cfg.clone();
            let extreme_band_picker = color_picker(
                data::config::theme::to_hsva(flow.extreme_band_color),
                move |value| {
                    let mut next = extreme_band_cfg.clone();
                    next.oi_flow_net.extreme_band_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            column![
                text("OI Flow Net").size(crate::style::text_size::SECTION),
                text(format!("Relative basis length: {}", flow.basis_length)),
                basis_length,
                text(format!(
                    "Flow burst sensitivity: {}",
                    flow.burst_sensitivity
                )),
                sensitivity,
                static_color,
                price_line,
                row![
                    text("Buy inflow"),
                    color_swatch(pane, flow.long_color, KlineColorPicker::OiFlowLong),
                    text("Sell inflow"),
                    color_swatch(pane, flow.short_color, KlineColorPicker::OiFlowShort),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text("Buy outflow"),
                    color_swatch(
                        pane,
                        flow.buy_outflow_color,
                        KlineColorPicker::OiFlowBuyOutflow
                    ),
                    text("Sell outflow"),
                    color_swatch(
                        pane,
                        flow.sell_outflow_color,
                        KlineColorPicker::OiFlowSellOutflow
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text("Extreme regions"),
                    color_swatch(
                        pane,
                        flow.extreme_band_color,
                        KlineColorPicker::OiFlowExtremeBand
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::OiFlowLong) {
                    long_picker
                } else if expanded == Some(KlineColorPicker::OiFlowShort) {
                    short_picker
                } else if expanded == Some(KlineColorPicker::OiFlowBuyOutflow) {
                    buy_outflow_picker
                } else if expanded == Some(KlineColorPicker::OiFlowSellOutflow) {
                    sell_outflow_picker
                } else if expanded == Some(KlineColorPicker::OiFlowExtremeBand) {
                    extreme_band_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::OiWeightedDelta => {
            let weighted = cfg.oi_weighted_delta;
            let sensitivity_cfg = cfg.clone();
            let sensitivity = slider(
                0.0..=50.0,
                weighted.sensitivity.clamp(0.0, 50.0),
                move |value| {
                    let mut next = sensitivity_cfg.clone();
                    next.oi_weighted_delta.sensitivity = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let max_cfg = cfg.clone();
            let max_weight = slider(
                1.0..=5.0,
                weighted.max_weight.clamp(1.0, 5.0),
                move |value| {
                    let mut next = max_cfg.clone();
                    next.oi_weighted_delta.max_weight = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.25);
            let width_cfg = cfg.clone();
            let width = slider(
                0.2..=0.95,
                weighted.bar_width.clamp(0.2, 0.95),
                move |value| {
                    let mut next = width_cfg.clone();
                    next.oi_weighted_delta.bar_width = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.05);
            let positive_cfg = cfg.clone();
            let positive_picker = color_picker(
                data::config::theme::to_hsva(weighted.positive_color),
                move |value| {
                    let mut next = positive_cfg.clone();
                    next.oi_weighted_delta.positive_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let negative_cfg = cfg.clone();
            let negative_picker = color_picker(
                data::config::theme::to_hsva(weighted.negative_color),
                move |value| {
                    let mut next = negative_cfg.clone();
                    next.oi_weighted_delta.negative_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            column![
                text("OI-Weighted Delta").size(crate::style::text_size::SECTION),
                text(format!("OI sensitivity: {:.0}", weighted.sensitivity)),
                sensitivity,
                text(format!("Maximum weight: {:.2}x", weighted.max_weight)),
                max_weight,
                text(format!("Bar width: {:.0}%", weighted.bar_width * 100.0)),
                width,
                row![
                    text("Positive"),
                    color_swatch(pane, weighted.positive_color, KlineColorPicker::OwdPositive),
                    text("Negative"),
                    color_swatch(pane, weighted.negative_color, KlineColorPicker::OwdNegative),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::OwdPositive) {
                    positive_picker
                } else if expanded == Some(KlineColorPicker::OwdNegative) {
                    negative_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::LiquidationPressureIndex => {
            let lpi = cfg.liquidation_pressure;
            let lookback_cfg = cfg.clone();
            let lookback = text_input("Lookback", &lpi.lookback.max(2).to_string())
                .on_input(move |value| {
                    let mut next = lookback_cfg.clone();
                    if let Ok(value) = value.parse::<u16>() {
                        next.liquidation_pressure.lookback = value.clamp(2, 200);
                    }
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                })
                .width(80);
            let oi_cfg = cfg.clone();
            let oi_threshold = slider(
                0.1..=10.0,
                lpi.oi_drop_threshold.clamp(0.1, 10.0),
                move |value| {
                    let mut next = oi_cfg.clone();
                    next.liquidation_pressure.oi_drop_threshold = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.1);
            let z_cfg = cfg.clone();
            let delta_zscore = slider(0.5..=5.0, lpi.delta_zscore.clamp(0.5, 5.0), move |value| {
                let mut next = z_cfg.clone();
                next.liquidation_pressure.delta_zscore = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.1);
            let width_cfg = cfg.clone();
            let width = slider(0.2..=0.95, lpi.bar_width.clamp(0.2, 0.95), move |value| {
                let mut next = width_cfg.clone();
                next.liquidation_pressure.bar_width = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.05);
            let positive_cfg = cfg.clone();
            let positive_picker = color_picker(
                data::config::theme::to_hsva(lpi.positive_color),
                move |value| {
                    let mut next = positive_cfg.clone();
                    next.liquidation_pressure.positive_color =
                        data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let negative_cfg = cfg.clone();
            let negative_picker = color_picker(
                data::config::theme::to_hsva(lpi.negative_color),
                move |value| {
                    let mut next = negative_cfg.clone();
                    next.liquidation_pressure.negative_color =
                        data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            column![
                text("Liquidation Pressure Index").size(crate::style::text_size::SECTION),
                row![text("Lookback bars"), lookback]
                    .spacing(8)
                    .align_y(Alignment::Center),
                text(format!("OI drop threshold: {:.1}%", lpi.oi_drop_threshold)),
                oi_threshold,
                text(format!("Delta z-score: {:.1}", lpi.delta_zscore)),
                delta_zscore,
                text(format!("Bar width: {:.0}%", lpi.bar_width * 100.0)),
                width,
                row![
                    text("Positive"),
                    color_swatch(pane, lpi.positive_color, KlineColorPicker::LpiPositive),
                    text("Negative"),
                    color_swatch(pane, lpi.negative_color, KlineColorPicker::LpiNegative),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::LpiPositive) {
                    positive_picker
                } else if expanded == Some(KlineColorPicker::LpiNegative) {
                    negative_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::OrderFlowBias => {
            use data::chart::kline::{OrderFlowBiasConfig, OrderFlowBiasPreset};

            let bias = cfg.order_flow_bias;
            let preset_cfg = cfg.clone();
            let preset = pick_list(OrderFlowBiasPreset::ALL, Some(bias.preset), move |preset| {
                let mut next = preset_cfg.clone();
                next.order_flow_bias = if preset == OrderFlowBiasPreset::Custom {
                    OrderFlowBiasConfig { preset, ..bias }
                } else {
                    OrderFlowBiasConfig::from_preset(preset)
                };
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            });

            let lookback_cfg = cfg.clone();
            let lookback = slider(
                2.0..=100.0,
                bias.lookback.clamp(2, 100) as f32,
                move |value| {
                    let mut next = lookback_cfg.clone();
                    next.order_flow_bias.lookback = value as u16;
                    next.order_flow_bias = next.order_flow_bias.custom();
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);

            let threshold_cfg = cfg.clone();
            let threshold = slider(
                5.0..=100.0,
                bias.signal_threshold.clamp(5.0, 100.0),
                move |value| {
                    let mut next = threshold_cfg.clone();
                    next.order_flow_bias.signal_threshold = value;
                    next.order_flow_bias = next.order_flow_bias.custom();
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(1.0);

            let delta_cfg = cfg.clone();
            let absorption_delta = slider(
                0.1..=0.95,
                bias.absorption_delta.clamp(0.1, 0.95),
                move |value| {
                    let mut next = delta_cfg.clone();
                    next.order_flow_bias.absorption_delta = value;
                    next.order_flow_bias = next.order_flow_bias.custom();
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.05);

            let response_cfg = cfg.clone();
            let price_response = slider(
                0.05..=1.0,
                bias.max_price_response.clamp(0.05, 1.0),
                move |value| {
                    let mut next = response_cfg.clone();
                    next.order_flow_bias.max_price_response = value;
                    next.order_flow_bias = next.order_flow_bias.custom();
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.05);

            let imbalance_cfg = cfg.clone();
            let imbalance_ratio = slider(
                1.1..=10.0,
                bias.imbalance_ratio.clamp(1.1, 10.0),
                move |value| {
                    let mut next = imbalance_cfg.clone();
                    next.order_flow_bias.imbalance_ratio = value;
                    next.order_flow_bias = next.order_flow_bias.custom();
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            )
            .step(0.1);

            column![
                text("Order Flow Bias").size(crate::style::text_size::SECTION),
                row![text("Preset"), preset]
                    .spacing(8)
                    .align_y(Alignment::Center),
                text(format!("Lookback: {} bars", bias.lookback)),
                lookback,
                text(format!("Signal threshold: {:.0}", bias.signal_threshold)),
                threshold,
                text(format!("Absorption Delta: {:.2}x", bias.absorption_delta)),
                absorption_delta,
                text(format!(
                    "Maximum price response: {:.0}%",
                    bias.max_price_response * 100.0
                )),
                price_response,
                text(format!("Imbalance ratio: {:.1}:1", bias.imbalance_ratio)),
                imbalance_ratio,
            ]
            .spacing(8)
            .into()
        }
        KlineIndicator::LiquidationHeatmap => {
            let heatmap = cfg.liquidation_heatmap;
            let row_cfg = cfg.clone();
            let row_height = slider(
                2.0..=24.0,
                heatmap.row_height.clamp(2.0, 24.0),
                move |value| {
                    let mut next = row_cfg.clone();
                    next.liquidation_heatmap.row_height = value;
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let opacity_cfg = cfg.clone();
            let opacity = slider(0.1..=1.0, heatmap.opacity.clamp(0.1, 1.0), move |value| {
                let mut next = opacity_cfg.clone();
                next.liquidation_heatmap.opacity = value;
                Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
            })
            .step(0.05);
            let long_cfg = cfg.clone();
            let long_picker = color_picker(
                data::config::theme::to_hsva(heatmap.long_color),
                move |value| {
                    let mut next = long_cfg.clone();
                    next.liquidation_heatmap.long_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            let short_cfg = cfg.clone();
            let short_picker = color_picker(
                data::config::theme::to_hsva(heatmap.short_color),
                move |value| {
                    let mut next = short_cfg.clone();
                    next.liquidation_heatmap.short_color = data::config::theme::from_hsva(value);
                    Message::VisualConfigChanged(pane, VisualConfig::Kline(next), false)
                },
            );
            column![
                text("Liquidation Heatmap").size(crate::style::text_size::SECTION),
                text(format!("Price row height: {:.0}px", heatmap.row_height)),
                row_height,
                text(format!("Opacity: {:.0}%", heatmap.opacity * 100.0)),
                opacity,
                row![
                    text("Long liquidations"),
                    color_swatch(
                        pane,
                        heatmap.long_color,
                        KlineColorPicker::LiquidationHeatmapLong
                    ),
                    text("Short liquidations"),
                    color_swatch(
                        pane,
                        heatmap.short_color,
                        KlineColorPicker::LiquidationHeatmapShort
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                if expanded == Some(KlineColorPicker::LiquidationHeatmapLong) {
                    long_picker
                } else if expanded == Some(KlineColorPicker::LiquidationHeatmapShort) {
                    short_picker
                } else {
                    space::vertical().height(0).into()
                },
            ]
            .spacing(8)
            .into()
        }
        _ => column![text("No settings available")].into(),
    };

    cfg_view_container(360, content)
}

fn color_swatch<'a>(
    pane: pane_grid::Pane,
    color: iced::Color,
    picker: KlineColorPicker,
) -> Element<'a, Message> {
    button(space::horizontal().width(28).height(18))
        .on_press(Message::PaneEvent(
            pane,
            Event::ToggleKlineColorPicker(picker),
        ))
        .style(move |_theme, _status| iced::widget::button::Style {
            background: Some(color.into()),
            border: iced::Border::default().rounded(2).width(1),
            ..Default::default()
        })
        .into()
}

pub fn ladder_cfg_view<'a>(cfg: ladder::Config, pane: pane_grid::Pane) -> Element<'a, Message> {
    let display_options = {
        let spread = checkbox(cfg.show_spread)
            .label("Show Spread")
            .on_toggle(move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Ladder(ladder::Config {
                        show_spread: value,
                        ..cfg
                    }),
                    false,
                )
            });

        let chase_tracker = checkbox(cfg.show_chase_tracker)
            .label("Show Chase Tracker")
            .on_toggle(move |value| {
                Message::VisualConfigChanged(
                    pane,
                    VisualConfig::Ladder(ladder::Config {
                        show_chase_tracker: value,
                        ..cfg
                    }),
                    false,
                )
            });

        column![
            text("Display Options").size(crate::style::text_size::SECTION),
            column![
                spread,
                row![
                    chase_tracker,
                    tooltip(
                        button("i").style(style::button::info),
                        Some("Highlights consecutive best-price moves and fades when momentum stalls.\nCalculated using raw ungrouped data."),
                        TooltipPosition::Top,
                    )
                ]
                .align_y(Alignment::Center)
                .spacing(4)
            ]
            .spacing(4)
        ]
        .spacing(8)
    };

    let retention_slider = {
        let retention_minutes = (cfg.trade_retention.as_secs_f32() / 60.0).max(1.0);

        let slider_ui = slider(1.0..=60.0, retention_minutes, move |new_minutes| {
            let mins = new_minutes.round().max(1.0) as u64;
            Message::VisualConfigChanged(
                pane,
                VisualConfig::Ladder(ladder::Config {
                    trade_retention: Duration::from_secs(mins * 60),
                    ..cfg
                }),
                false,
            )
        })
        .step(1.0);

        classic_slider_row(
            text("Keep trades for"),
            slider_ui.into(),
            Some(
                text(format!("≈ {} min", retention_minutes.round() as u64))
                    .size(crate::style::text_size::EMPHASIS),
            ),
        )
    };

    let history_column = column![
        text("History").size(crate::style::text_size::SECTION),
        retention_slider
    ]
    .spacing(8);

    let content = split_column![
        display_options,
        history_column,
        row![
            space::horizontal(),
            sync_all_button(pane, VisualConfig::Ladder(cfg))
        ],
        ; spacing = 12, align_x = Alignment::Start
    ];

    cfg_view_container(320, content)
}

fn sync_all_button<'a>(pane: pane_grid::Pane, config: VisualConfig) -> Element<'a, Message> {
    tooltip(
        button("Sync all").on_press(Message::VisualConfigChanged(pane, config, true)),
        Some("Apply configuration to similar panes"),
        TooltipPosition::Top,
    )
}

pub mod study {
    use crate::{
        split_column,
        style::{self, Icon, icon_text},
    };
    use data::chart::heatmap::{CLEANUP_THRESHOLD, HeatmapStudy, ProfileKind};
    use data::chart::kline::FootprintStudy;
    use iced::{
        Element, padding,
        widget::{button, checkbox, column, container, row, slider, space, text},
    };

    #[derive(Debug, Clone, Copy)]
    pub enum StudyMessage {
        Footprint(Message<FootprintStudy>),
        Heatmap(Message<HeatmapStudy>),
    }

    pub trait Study: Sized + Copy + 'static {
        fn is_same_type(&self, other: &Self) -> bool;
        fn all() -> Vec<Self>;
        fn view_config<'a>(
            &self,
            basis: data::chart::Basis,
            on_change: impl Fn(Self) -> Message<Self> + Copy + 'a,
        ) -> Element<'a, Message<Self>>;
    }

    impl Study for FootprintStudy {
        fn is_same_type(&self, other: &Self) -> bool {
            std::mem::discriminant(self) == std::mem::discriminant(other)
        }

        fn all() -> Vec<Self> {
            FootprintStudy::ALL.to_vec()
        }

        fn view_config<'a>(
            &self,
            _basis: data::chart::Basis,
            on_change: impl Fn(Self) -> Message<Self> + Copy + 'a,
        ) -> Element<'a, Message<Self>> {
            match *self {
                FootprintStudy::NPoC { lookback } => {
                    let slider_ui = slider(10.0..=400.0, lookback as f32, move |new_value| {
                        on_change(FootprintStudy::NPoC {
                            lookback: new_value as usize,
                        })
                    })
                    .step(10.0);

                    column![text(format!("Lookback: {lookback} datapoints")), slider_ui]
                        .padding(8)
                        .spacing(4)
                        .into()
                }
                FootprintStudy::Imbalance {
                    threshold,
                    color_scale,
                    ignore_zeros,
                } => {
                    let qty_threshold = {
                        let info_text = text(format!("Ask:Bid threshold: {threshold}%"));

                        let threshold_slider =
                            slider(100.0..=800.0, threshold as f32, move |new_value| {
                                on_change(FootprintStudy::Imbalance {
                                    threshold: new_value as usize,
                                    color_scale,
                                    ignore_zeros,
                                })
                            })
                            .step(25.0);

                        column![info_text, threshold_slider,].padding(8).spacing(4)
                    };

                    let color_scaling = {
                        let color_scale_enabled = color_scale.is_some();
                        let color_scale_value = color_scale.unwrap_or(100);

                        let color_scale_checkbox = checkbox(color_scale_enabled)
                            .label("Dynamic color scaling")
                            .on_toggle(move |is_enabled| {
                                on_change(FootprintStudy::Imbalance {
                                    threshold,
                                    color_scale: if is_enabled {
                                        Some(color_scale_value)
                                    } else {
                                        None
                                    },
                                    ignore_zeros,
                                })
                            });

                        if color_scale_enabled {
                            let scaling_slider = column![
                                text(format!("Opaque color at: {color_scale_value}x")),
                                slider(50.0..=2000.0, color_scale_value as f32, move |new_value| {
                                    on_change(FootprintStudy::Imbalance {
                                        threshold,
                                        color_scale: Some(new_value as usize),
                                        ignore_zeros,
                                    })
                                })
                                .step(50.0)
                            ]
                            .spacing(2);

                            column![color_scale_checkbox, scaling_slider]
                                .padding(8)
                                .spacing(8)
                        } else {
                            column![color_scale_checkbox].padding(8)
                        }
                    };

                    let ignore_zeros_checkbox = {
                        let cbox = checkbox(ignore_zeros).label("Ignore zeros").on_toggle(
                            move |is_checked| {
                                on_change(FootprintStudy::Imbalance {
                                    threshold,
                                    color_scale,
                                    ignore_zeros: is_checked,
                                })
                            },
                        );

                        column![cbox].padding(8).spacing(4)
                    };

                    split_column![qty_threshold, color_scaling, ignore_zeros_checkbox]
                        .padding(4)
                        .into()
                }
            }
        }
    }

    impl Study for HeatmapStudy {
        fn is_same_type(&self, other: &Self) -> bool {
            std::mem::discriminant(self) == std::mem::discriminant(other)
        }

        fn all() -> Vec<Self> {
            HeatmapStudy::ALL.to_vec()
        }

        fn view_config<'a>(
            &self,
            basis: data::chart::Basis,
            on_change: impl Fn(Self) -> Message<Self> + Copy + 'a,
        ) -> Element<'a, Message<Self>> {
            let interval_ms = match basis {
                data::chart::Basis::Time(interval) => interval.to_milliseconds(),
                data::chart::Basis::Tick(_) => {
                    return iced::widget::center(text(
                        "Heatmap studies are not supported for tick-based charts",
                    ))
                    .into();
                }
            };

            match self {
                HeatmapStudy::VolumeProfile(kind) => match kind {
                    ProfileKind::FixedWindow(datapoint_count) => {
                        let duration_secs = (*datapoint_count as u64 * interval_ms) / 1000;
                        let min_range = CLEANUP_THRESHOLD / 20;

                        let duration_text = if duration_secs < 60 {
                            format!("{} seconds", duration_secs)
                        } else {
                            let minutes = duration_secs / 60;
                            let seconds = duration_secs % 60;
                            if seconds == 0 {
                                format!("{} minutes", minutes)
                            } else {
                                format!("{}m {}s", minutes, seconds)
                            }
                        };

                        let slider = slider(
                            min_range as f32..=CLEANUP_THRESHOLD as f32,
                            *datapoint_count as f32,
                            move |new_datapoint_count| {
                                on_change(HeatmapStudy::VolumeProfile(ProfileKind::FixedWindow(
                                    new_datapoint_count as usize,
                                )))
                            },
                        )
                        .step(40.0);

                        let switch_kind = button(text("Switch to visible range")).on_press(
                            on_change(HeatmapStudy::VolumeProfile(ProfileKind::VisibleRange)),
                        );

                        column![
                            row![space::horizontal(), switch_kind,],
                            text(format!(
                                "Window: {} datapoints ({})",
                                datapoint_count, duration_text
                            )),
                            slider,
                        ]
                        .padding(8)
                        .spacing(4)
                        .into()
                    }
                    ProfileKind::VisibleRange => {
                        let switch_kind = button(text("Switch to fixed window")).on_press(
                            on_change(HeatmapStudy::VolumeProfile(ProfileKind::FixedWindow(
                                CLEANUP_THRESHOLD / 5_usize,
                            ))),
                        );

                        column![row![space::horizontal(), switch_kind,],]
                            .padding(8)
                            .spacing(4)
                            .into()
                    }
                },
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub enum Message<S: Study> {
        CardToggled(S),
        StudyToggled(S, bool),
        StudyValueChanged(S),
    }

    pub enum Action<S: Study> {
        ToggleStudy(S, bool),
        ConfigureStudy(S),
    }

    pub struct Configurator<S: Study> {
        expanded_card: Option<S>,
    }

    impl<S: Study> Default for Configurator<S> {
        fn default() -> Self {
            Self {
                expanded_card: None,
            }
        }
    }

    impl<S: Study + ToString> Configurator<S> {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn update(&mut self, message: Message<S>) -> Option<Action<S>> {
            match message {
                Message::CardToggled(study) => {
                    let should_collapse = self
                        .expanded_card
                        .as_ref()
                        .is_some_and(|expanded| expanded.is_same_type(&study));

                    if should_collapse {
                        self.expanded_card = None;
                    } else {
                        self.expanded_card = Some(study);
                    }
                }
                Message::StudyToggled(study, is_checked) => {
                    return Some(Action::ToggleStudy(study, is_checked));
                }
                Message::StudyValueChanged(study) => {
                    return Some(Action::ConfigureStudy(study));
                }
            }

            None
        }

        pub fn view<'a>(
            &self,
            active_studies: &[S],
            basis: data::chart::Basis,
        ) -> Element<'a, Message<S>> {
            self.view_available(active_studies, basis, S::all())
        }

        pub fn view_available<'a>(
            &self,
            active_studies: &[S],
            basis: data::chart::Basis,
            available_studies: Vec<S>,
        ) -> Element<'a, Message<S>> {
            let mut content = column![].spacing(4);

            for available_study in available_studies {
                content =
                    content.push(self.create_study_row(available_study, active_studies, basis));
            }

            content.into()
        }

        fn create_study_row<'a>(
            &self,
            study: S,
            active_studies: &[S],
            basis: data::chart::Basis,
        ) -> Element<'a, Message<S>> {
            let (is_selected, study_config) = {
                let mut is_selected = false;
                let mut study_config = None;

                for s in active_studies {
                    if s.is_same_type(&study) {
                        is_selected = true;
                        study_config = Some(*s);
                        break;
                    }
                }
                (is_selected, study_config)
            };

            let checkbox = checkbox(is_selected)
                .label(study_config.map_or(study.to_string(), |s| s.to_string()))
                .on_toggle(move |checked| Message::StudyToggled(study, checked));

            let mut checkbox_row = row![checkbox, space::horizontal()]
                .height(36)
                .align_y(iced::Alignment::Center)
                .padding(padding::left(8).right(4))
                .spacing(4);

            let is_expanded = self
                .expanded_card
                .as_ref()
                .is_some_and(|expanded| expanded.is_same_type(&study));

            if is_selected {
                checkbox_row = checkbox_row.push(
                    button(icon_text(Icon::Cog, 12))
                        .on_press(Message::CardToggled(study))
                        .style(move |theme, status| {
                            style::button::transparent(theme, status, is_expanded)
                        }),
                );
            }

            let mut column = column![checkbox_row];

            if is_expanded && let Some(config) = study_config {
                column = column.push(config.view_config(basis, Message::StudyValueChanged));
            }

            container(column).style(style::modal_container).into()
        }
    }
}
