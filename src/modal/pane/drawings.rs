use crate::chart::DrawingTool;
use crate::screen::dashboard::pane::{self, Message};
use crate::style::{self, Icon, LucideIcon, icon_text};
use iced::{
    Element, Length, padding,
    widget::{button, column, container, pane_grid, row, space, text},
};

pub fn view(pane: pane_grid::Pane, selected: Option<DrawingTool>) -> Element<'static, Message> {
    container(
        column![
            text("Drawing tools").size(crate::style::text_size::SECTION),
            section_label("Lines"),
            tool_row(
                pane,
                DrawingTool::TrendLine,
                LucideIcon::TrendingUp,
                selected,
            ),
            tool_row(
                pane,
                DrawingTool::HorizontalLine,
                LucideIcon::Minus,
                selected,
            ),
            section_label("Profiles"),
            tool_row(
                pane,
                DrawingTool::VolumeProfile,
                LucideIcon::ChartNoAxesColumnIncreasing,
                selected,
            ),
        ]
        .spacing(4),
    )
    .width(260)
    .max_width(260)
    .padding(16)
    .style(style::chart_modal)
    .into()
}

fn section_label(label: &'static str) -> Element<'static, Message> {
    container(text(label).size(crate::style::text_size::SMALL))
        .padding(padding::top(8).bottom(2))
        .into()
}

fn tool_row(
    pane: pane_grid::Pane,
    tool: DrawingTool,
    icon: LucideIcon,
    selected: Option<DrawingTool>,
) -> Element<'static, Message> {
    let is_selected = selected == Some(tool);
    let mut content = row![
        style::lucide_icon_text(icon, 13),
        text(tool.to_string()),
        space::horizontal(),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .width(Length::Fill);
    if is_selected {
        content = content.push(icon_text(Icon::Checkmark, 12));
    }

    button(content)
        .on_press(Message::PaneEvent(
            pane,
            pane::Event::DrawingToolSelected(Some(tool)),
        ))
        .width(Length::Fill)
        .style(move |theme, status| style::button::menu_body(theme, status, is_selected))
        .into()
}
