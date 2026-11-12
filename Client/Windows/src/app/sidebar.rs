//! Left sidebar: server header (menu), channel tree with drag & drop,
//! voice status panel and user panel.

use super::theme::{self, IconState};
use super::*;

/// Drag payload: a channel or category being moved.
#[derive(Clone, Copy, Debug)]
pub(super) struct ChannelDrag {
    pub id: u64,
    pub is_category: bool,
}

/// A possible drop position, collected while drawing the tree.
#[derive(Clone, Copy, Debug)]
struct DropSlot {
    y: f32,
    indent: f32,
    parent: Option<u64>,
    index: u32,
}

const SIDEBAR_MIN: f32 = 220.0;
const SIDEBAR_MAX: f32 = 480.0;

fn row_h() -> f32 {
    theme::dense(30.0, 24.0)
}
/// Width of the server dropdown menu.
const MENU_W: f32 = 230.0;

impl RustTalkApp {
    /// Children of `parent` (without the lobby), in display order.
    pub(super) fn sorted_children(&self, parent: Option<u64>) -> Vec<ChannelInfo> {
        let mut v: Vec<ChannelInfo> = self
            .channels
            .values()
            .filter(|c| c.parent == parent && c.kind != ChannelKind::Spawn)
            .cloned()
            .collect();
        v.sort_by_key(|c| (c.position, c.id));
        v
    }

    pub(super) fn my_channel(&self) -> Option<u64> {
        self.users.get(&self.my_id).map(|u| u.channel_id)
    }

    pub(super) fn in_lobby(&self) -> bool {
        self.my_channel().is_some_and(|c| Some(c) == self.spawn_channel_id())
    }

    pub(super) fn ui_sidebar(
        &mut self,
        ctx: &egui::Context,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        let width = self.config.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let panel = egui::SidePanel::left("sidebar")
            .default_width(width)
            .width_range(SIDEBAR_MIN..=SIDEBAR_MAX)
            .resizable(true)
            .frame(egui::Frame::default().fill(theme::SIDEBAR))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::TopBottomPanel::top("server_header")
                    .exact_height(48.0)
                    .frame(
                        egui::Frame::default()
                            .fill(theme::SIDEBAR)
                            .inner_margin(egui::Margin::symmetric(12, 0))
                            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER)),
                    )
                    .show_inside(ui, |ui| self.ui_server_header(ui));

                egui::TopBottomPanel::bottom("user_panel")
                    .frame(egui::Frame::default().fill(theme::SIDEBAR).inner_margin(egui::Margin::same(8)))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        egui::Frame::default()
                            .fill(theme::PANEL)
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 4.0;
                                self.ui_voice_panel(ui, cmd_tx);
                                self.ui_user_panel(ui, cmd_tx);
                            });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 10,
                        bottom: 8,
                    }))
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.ui_channel_tree(ui, cmd_tx));
                    });
            });
        // Persist once the drag is over (not every frame while dragging).
        let new = panel.response.rect.width().round();
        if (new - self.config.sidebar_width).abs() >= 1.0 && !ctx.input(|i| i.pointer.any_down()) {
            self.config.sidebar_width = new;
            self.config.save();
        }
    }

    /// Server icon + name with dropdown menu (server settings, create, ...).
    fn ui_server_header(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("server_menu_btn"), egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect.shrink2(egui::vec2(-12.0, 0.0)), 0.0, theme::HOVER);
        }
        let popup_id = ui.make_persistent_id("server_menu");
        let open = ui.memory(|m| m.is_popup_open(popup_id));
        let icon_rect = egui::Rect::from_center_size(rect.left_center() + egui::vec2(14.0, 0.0), egui::vec2(28.0, 28.0));
        let settings = self.server_settings.clone();
        server_settings::paint_server_icon(ui, icon_rect, &settings, &mut self.textures);
        let font = egui::FontId::proportional(15.5);
        let name = elide(ui, &settings.name, &font, rect.width() - 64.0);
        ui.painter().text(
            rect.left_center() + egui::vec2(36.0, 0.0),
            egui::Align2::LEFT_CENTER,
            name,
            font,
            theme::HEADER,
        );
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            if open { icons::regular::X } else { icons::regular::CARET_DOWN },
            egui::FontId::proportional(16.0),
            theme::HEADER,
        );
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        egui::popup_below_widget(
            ui,
            popup_id,
            &resp,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                // Fixed width: items sized from available_width() made the
                // popup grow every frame.
                ui.set_width(MENU_W);
                ui.spacing_mut().item_spacing.y = 2.0;
                if theme::menu_item(ui, icons::regular::SHIELD_CHECK, "Server Settings", false).clicked() {
                    self.open_server_settings(server_settings::ServerTab::Overview);
                }
                theme::menu_separator(ui);
                let admin = self.can(rusttalk_shared::protocol::Role::Admin);
                ui.add_enabled_ui(admin, |ui| {
                    if theme::menu_item(ui, icons::regular::PLUS_CIRCLE, "Create Channel", false).on_disabled_hover_text("Only admins").clicked() {
                        self.open_new_channel(None, ChannelKind::Text);
                    }
                    if theme::menu_item(ui, icons::regular::FOLDER_PLUS, "Create Category", false).on_disabled_hover_text("Only admins").clicked() {
                        self.open_new_channel(None, ChannelKind::Category);
                    }
                });
                theme::menu_separator(ui);
                if theme::menu_item(ui, icons::regular::GEAR_SIX, "App Settings", false).clicked() {
                    self.open_settings(SettingsPage::Voice);
                }
                if theme::menu_item(ui, icons::regular::BUG, "Report a Bug", false).clicked() {
                    server_settings::open_bug_report(ui.ctx());
                }
            },
        );
        resp.on_hover_text(format!("{}  -  {}", settings.name, self.config.server_address));
    }

    /// Channel tree: lobby pinned on top, then top-level channels and
    /// categories in server order. Rows can be dragged to reorder.
    fn ui_channel_tree(
        &mut self,
        ui: &mut egui::Ui,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        ui.spacing_mut().item_spacing.y = 2.0;
        let ctx = ui.ctx().clone();
        let drag = egui::DragAndDrop::payload::<ChannelDrag>(&ctx).map(|d| *d);
        let dragged = drag.map(|d| d.id);
        let users = self.users.clone();
        let talking: HashMap<u64, Instant> = self.talking.lock().unwrap().clone();
        let local_muted: HashSet<u64> = self.local_muted.lock().unwrap().clone();

        // Lobby (pinned, not draggable).
        if let Some(spawn) = self.spawn_channel_id().and_then(|id| self.channels.get(&id).cloned()) {
            self.channel_row(ui, &spawn, &users, &talking, &local_muted, cmd_tx, 0.0, false);
            ui.add_space(6.0);
        }

        let tree_left = ui.max_rect().left();
        let mut slots: Vec<DropSlot> = Vec::new();
        // (id, top, bottom) of each top-level block (category incl. children).
        let mut top_blocks: Vec<(u64, f32, f32)> = Vec::new();

        for item in self.sorted_children(None) {
            let top = ui.cursor().top();
            if item.kind == ChannelKind::Category {
                let collapsed = self.collapsed.get(&item.id).copied().unwrap_or(false);
                let header = self.category_header(ui, &item, collapsed, dragged == Some(item.id));
                let children: Vec<ChannelInfo> = self.sorted_children(Some(item.id));
                let mut child_rects: Vec<(u64, egui::Rect)> = Vec::new();
                if !collapsed {
                    for ch in &children {
                        let r = self.channel_row(
                            ui,
                            ch,
                            &users,
                            &talking,
                            &local_muted,
                            cmd_tx,
                            0.0,
                            dragged == Some(ch.id),
                        );
                        child_rects.push((ch.id, r));
                    }
                }
                // Slots inside this category (only for non-category drags).
                if drag.is_some_and(|d| !d.is_category) {
                    let visible: Vec<&(u64, egui::Rect)> =
                        child_rects.iter().filter(|(id, _)| Some(*id) != dragged).collect();
                    if collapsed {
                        let n = children.iter().filter(|c| Some(c.id) != dragged).count();
                        slots.push(DropSlot {
                            y: header.bottom(),
                            indent: 16.0,
                            parent: Some(item.id),
                            index: n as u32,
                        });
                    } else {
                        for (k, (_, r)) in visible.iter().enumerate() {
                            slots.push(DropSlot {
                                y: r.top(),
                                indent: 16.0,
                                parent: Some(item.id),
                                index: k as u32,
                            });
                        }
                        let end_y = visible.last().map(|(_, r)| r.bottom()).unwrap_or(header.bottom());
                        slots.push(DropSlot {
                            y: end_y,
                            indent: 16.0,
                            parent: Some(item.id),
                            index: visible.len() as u32,
                        });
                    }
                }
                ui.add_space(8.0);
            } else {
                self.channel_row(ui, &item, &users, &talking, &local_muted, cmd_tx, 0.0, dragged == Some(item.id));
            }
            top_blocks.push((item.id, top, ui.cursor().top()));
        }

        // Top-level slots (between blocks), valid for every drag.
        if drag.is_some() {
            let visible: Vec<&(u64, f32, f32)> =
                top_blocks.iter().filter(|(id, _, _)| Some(*id) != dragged).collect();
            for (k, (_, top, _)) in visible.iter().enumerate() {
                slots.push(DropSlot { y: *top, indent: 0.0, parent: None, index: k as u32 });
            }
            let end_y = visible.last().map(|(_, _, b)| *b).unwrap_or(ui.cursor().top());
            slots.push(DropSlot { y: end_y, indent: 0.0, parent: None, index: visible.len() as u32 });
        }

        // Room at the end so you can drop below the last item.
        let (rest, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
        let tree_rect = egui::Rect::from_min_max(
            egui::pos2(tree_left, ui.min_rect().top()),
            egui::pos2(ui.max_rect().right(), rest.bottom()),
        );

        if let (Some(d), Some(pointer)) = (drag, ctx.pointer_latest_pos()) {
            self.drag_feedback(ui, &ctx, d, pointer, tree_rect, &slots, cmd_tx);
        }
    }

    /// Ghost label, insertion line and drop handling for a running drag.
    #[allow(clippy::too_many_arguments)]
    fn drag_feedback(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        drag: ChannelDrag,
        pointer: egui::Pos2,
        tree_rect: egui::Rect,
        slots: &[DropSlot],
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let name = self.channel_name(drag.id);
        let icon = self.channels.get(&drag.id).map(|c| channel_icon(c.kind)).unwrap_or("");
        egui::Area::new(egui::Id::new("drag_ghost"))
            .fixed_pos(pointer + egui::vec2(14.0, 6.0))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::DARK)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(format!("{icon}  {name}")).color(theme::HEADER));
                    });
            });

        let inside = tree_rect.expand(20.0).contains(pointer);
        // Nearest slot; slight preference for "inside category" slots when
        // the pointer is indented (Discord-like feel).
        let indented = pointer.x > tree_rect.left() + 24.0;
        let best = slots
            .iter()
            .map(|s| {
                let mut d = (s.y - pointer.y).abs();
                if (s.indent > 0.0) != indented {
                    d += 6.0;
                }
                (d, s)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, s)| *s);

        if let (true, Some(slot)) = (inside, best) {
            let x0 = tree_rect.left() + slot.indent;
            let p = ui.painter();
            p.line_segment(
                [egui::pos2(x0, slot.y), egui::pos2(tree_rect.right() - 4.0, slot.y)],
                egui::Stroke::new(2.0_f32, theme::accent()),
            );
            p.circle_filled(egui::pos2(x0, slot.y), 3.5, theme::accent());

            if ctx.input(|i| i.pointer.any_released()) {
                let _ = cmd_tx.send(Command::MoveChannel {
                    channel_id: drag.id,
                    parent: slot.parent,
                    index: slot.index,
                });
                egui::DragAndDrop::clear_payload(ctx);
            }
        }
        if ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ctx);
        }
    }

    /// Category header: caret + uppercase name; hover shows "+".
    fn category_header(
        &mut self,
        ui: &mut egui::Ui,
        cat: &ChannelInfo,
        collapsed: bool,
        is_dragged: bool,
    ) -> egui::Rect {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_h()),
            egui::Sense::click_and_drag(),
        );
        let hovered = resp.hovered() || resp.dragged();
        // The "+"/gear stay while the pointer is on the row - also during the
        // click itself, when egui no longer counts the (draggable) row as hovered.
        let show_actions = !is_dragged && pointer_on(ui, rect);
        let has_unread = self
            .channels
            .values()
            .any(|c| c.parent == Some(cat.id) && self.unread.contains(&c.id));
        let color = if is_dragged {
            theme::DISABLED
        } else if hovered || has_unread {
            theme::HEADER
        } else {
            theme::MUTED
        };
        let p = ui.painter();
        p.text(
            rect.left_center() + egui::vec2(2.0, 0.0),
            egui::Align2::LEFT_CENTER,
            if collapsed { icons::regular::CARET_RIGHT } else { icons::regular::CARET_DOWN },
            egui::FontId::proportional(11.0),
            color,
        );
        p.text(
            rect.left_center() + egui::vec2(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            cat.name.to_uppercase(),
            egui::FontId::proportional(12.0),
            color,
        );

        // Hover actions: "+" (new channel in this category), gear (settings).
        let mut action_hit = false;
        let admin = self.can(rusttalk_shared::protocol::Role::Admin);
        if show_actions {
            let plus = egui::Rect::from_center_size(rect.right_center() - egui::vec2(12.0, 0.0), egui::vec2(22.0, 22.0));
            let gear = egui::Rect::from_center_size(rect.right_center() - egui::vec2(36.0, 0.0), egui::vec2(22.0, 22.0));
            if admin && small_icon(ui, plus, icons::regular::PLUS, "Create channel in this category").clicked() {
                self.open_new_channel(Some(cat.id), ChannelKind::Text);
                action_hit = true;
            }
            if small_icon(ui, gear, icons::regular::GEAR_SIX, "Edit category").clicked() {
                self.open_channel_settings(cat.id);
                action_hit = true;
            }
        }
        if resp.clicked() && !action_hit {
            self.collapsed.insert(cat.id, !collapsed);
        }
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), ChannelDrag { id: cat.id, is_category: true });
        }
        resp.context_menu(|ui| {
            ui.set_width(MENU_W);
            ui.spacing_mut().item_spacing.y = 2.0;
            if admin && theme::menu_item(ui, icons::regular::PLUS_CIRCLE, "Create Channel Here", false).clicked() {
                self.open_new_channel(Some(cat.id), ChannelKind::Text);
                ui.close_menu();
            }
            if theme::menu_item(ui, icons::regular::GEAR_SIX, "Edit Category", false).clicked() {
                self.open_channel_settings(cat.id);
                ui.close_menu();
            }
            theme::menu_separator(ui);
            if theme::menu_item(ui, icons::regular::TRASH, "Delete Category", true)
                .clicked()
            {
                self.confirm_delete = Some(cat.id);
                ui.close_menu();
            }
        });
        rect
    }

    /// One channel row (+ its members for voice/lobby). Returns the row rect.
    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &mut self,
        ui: &mut egui::Ui,
        ch: &ChannelInfo,
        users: &HashMap<u64, UserInfo>,
        talking: &HashMap<u64, Instant>,
        local_muted: &HashSet<u64>,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
        indent: f32,
        is_dragged: bool,
    ) -> egui::Rect {
        let selected = self.selected_channel == Some(ch.id);
        let i_am_here = self.my_channel() == Some(ch.id);
        let has_unread = self.unread.contains(&ch.id) && !selected;
        let draggable = ch.kind != ChannelKind::Spawn;

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_h()),
            if draggable { egui::Sense::click_and_drag() } else { egui::Sense::click() },
        );
        let hovered = resp.hovered() && !is_dragged;
        let bg = if selected {
            theme::SELECTED
        } else if hovered {
            theme::HOVER
        } else {
            egui::Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect_filled(rect.shrink2(egui::vec2(0.0, 1.0)), 6.0, bg);
        if has_unread {
            // Unread marker: small white pill on the left edge.
            let pill = egui::Rect::from_center_size(
                egui::pos2(rect.left() - 6.0, rect.center().y),
                egui::vec2(4.0, 8.0),
            );
            p.rect_filled(pill, 2.0, theme::HEADER);
        }
        let fg = if is_dragged {
            theme::DISABLED
        } else if selected || has_unread || hovered {
            theme::HEADER
        } else {
            theme::MUTED
        };
        let x = rect.left() + 8.0 + indent;
        p.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            channel_icon(ch.kind),
            egui::FontId::proportional(17.0),
            if i_am_here { theme::GREEN } else { fg },
        );
        let name_font = egui::FontId::proportional(14.5);
        let is_lobby = ch.kind == ChannelKind::Spawn;
        let max_name_w = rect.width() - 34.0 - indent - if hovered { 30.0 } else { 6.0 } - if is_lobby { 26.0 } else { 0.0 };
        let name = elide(ui, &ch.name, &name_font, max_name_w);
        p.text(
            egui::pos2(x + 24.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            name_font,
            if has_unread { theme::HEADER } else { fg },
        );

        // Hover: gear (channel settings) - by pointer position, see the category row.
        let mut action_hit = false;
        // Lobby row: always-visible server disconnect on the right.
        let lobby_shift = if is_lobby {
            let btn = egui::Rect::from_center_size(rect.right_center() - egui::vec2(14.0, 0.0), egui::vec2(22.0, 22.0));
            let tip = format!("Disconnect from {}", self.server_settings.name);
            if disconnect_icon(ui, btn, &tip).clicked() {
                self.pending_disconnect = true;
                action_hit = true;
            }
            26.0
        } else {
            0.0
        };
        if !is_dragged && pointer_on(ui, rect) {
            let gear = egui::Rect::from_center_size(rect.right_center() - egui::vec2(14.0 + lobby_shift, 0.0), egui::vec2(22.0, 22.0));
            if small_icon(ui, gear, icons::regular::GEAR_SIX, "Channel settings").clicked() {
                self.open_channel_settings(ch.id);
                action_hit = true;
            }
        }

        // Click = open the channel (members + chat); joining voice is a
        // deliberate double-click (or "Join" in the menu / the header button).
        if resp.clicked() && !action_hit {
            self.select_channel(ch.id);
        }
        if resp.double_clicked() && !action_hit && ch.kind.can_join() && !i_am_here {
            let _ = cmd_tx.send(Command::JoinChannel(ch.id));
        }
        if draggable && resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), ChannelDrag { id: ch.id, is_category: false });
        }
        let hover_text = match ch.kind {
            ChannelKind::Voice if i_am_here => "You are here",
            ChannelKind::Voice => "Double-click to join  -  drag to move",
            ChannelKind::Spawn if i_am_here => "Lobby: everyone lands here, talking is disabled",
            ChannelKind::Spawn => "Double-click to go back to the lobby (talking is disabled there)",
            _ => "Click to open  -  drag to move",
        };
        let resp = resp.on_hover_text_at_pointer(hover_text);
        resp.context_menu(|ui| self.channel_context_menu(ui, ch, cmd_tx));

        // Members (voice channels and lobby).
        if ch.kind.can_join() {
            for uid in &ch.user_ids {
                if let Some(user) = users.get(uid) {
                    self.user_row(ui, user, talking, local_muted, ch.kind == ChannelKind::Spawn);
                }
            }
        }
        rect
    }

    /// Right-click menu of a channel.
    fn channel_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        ch: &ChannelInfo,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing.y = 2.0;
        if ch.kind.can_join() && self.my_channel() != Some(ch.id) {
            let label = if ch.kind == ChannelKind::Spawn { "Back to Lobby" } else { "Join" };
            if theme::menu_item(ui, icons::regular::SIGN_IN, label, false).clicked() {
                let _ = cmd_tx.send(Command::JoinChannel(ch.id));
                self.select_channel(ch.id);
                ui.close_menu();
            }
        }
        if ch.kind.has_chat() && theme::menu_item(ui, icons::regular::CHAT_CIRCLE_TEXT, "Open Chat", false).clicked() {
            self.select_channel(ch.id);
            ui.close_menu();
        }
        if theme::menu_item(ui, icons::regular::CHECKS, "Mark as Read", false).clicked() {
            self.unread.remove(&ch.id);
            ui.close_menu();
        }
        theme::menu_separator(ui);
        if theme::menu_item(ui, icons::regular::GEAR_SIX, "Edit Channel", false).clicked() {
            self.open_channel_settings(ch.id);
            ui.close_menu();
        }
        if ch.kind != ChannelKind::Spawn {
            if theme::menu_item(ui, icons::regular::TRASH, "Delete Channel", true)
                .clicked()
            {
                self.confirm_delete = Some(ch.id);
                ui.close_menu();
            }
        }
    }

    /// Member under a voice channel: avatar (green ring/dot when talking),
    /// name, mute icons; right-click: local mute + per-user volume.
    fn user_row(
        &mut self,
        ui: &mut egui::Ui,
        user: &UserInfo,
        talking: &HashMap<u64, Instant>,
        local_muted: &HashSet<u64>,
        in_lobby: bool,
    ) {
        let me = user.id == self.my_id;
        let talking_now = !in_lobby
            && (talking.get(&user.id).is_some_and(|t| t.elapsed() < Duration::from_millis(250))
                || (me && self.control.transmitting.load(Ordering::Relaxed)));
        let locally_muted = local_muted.contains(&user.id);

        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::dense(34.0, 22.0)), egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, theme::HOVER);
        }
        let d = theme::dense(24.0, 18.0);
        let av = egui::Rect::from_center_size(rect.left_center() + egui::vec2(40.0, 0.0), egui::vec2(d, d));
        theme::paint_avatar(ui.painter(), av, &user.name, talking_now);
        let name_color = if locally_muted {
            theme::DISABLED
        } else if talking_now || me {
            theme::HEADER
        } else {
            theme::MUTED
        };
        ui.painter().text(
            egui::pos2(av.right() + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &user.name,
            egui::FontId::proportional(13.5),
            name_color,
        );
        // Status icons right-aligned.
        let mut x = rect.right() - 8.0;
        let mut icon_at = |icon: &str, color: egui::Color32, tip: &str| {
            let r = egui::Rect::from_center_size(egui::pos2(x - 8.0, rect.center().y), egui::vec2(16.0, 16.0));
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(14.0), color);
            ui.interact(r, ui.id().with(("ustat", user.id, tip)), egui::Sense::hover()).on_hover_text(tip);
            x -= 20.0;
        };
        if user.deafened {
            icon_at(icons::regular::SPEAKER_SLASH, theme::RED, "Deafened");
        }
        if user.muted {
            icon_at(icons::regular::MICROPHONE_SLASH, theme::RED, "Muted");
        }
        if locally_muted {
            icon_at(icons::regular::SPEAKER_NONE, theme::YELLOW, "Muted for you");
        }
        if user.sharing {
            icon_at(icons::regular::BROADCAST, theme::GREEN, "Live (camera or screen)");
        }
        if user.role != rusttalk_shared::protocol::Role::Member {
            icon_at(icons::regular::SHIELD_CHECK, super::moderation_ui::role_color(user.role), user.role.label());
        }

        if !me {
            resp.context_menu(|ui| self.user_menu(ui, user));
        }
    }

    /// Context menu of another user (sidebar, voice stage tiles, "..." button):
    /// poke, local mute, per-user volume, moderation.
    pub(super) fn user_menu(&mut self, ui: &mut egui::Ui, user: &UserInfo) {
        ui.set_width(MENU_W);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(&user.name).strong().color(theme::HEADER));
        theme::menu_separator(ui);
        if theme::menu_item(ui, icons::regular::HAND_WAVING, "Poke...", false)
            .on_hover_text("Plays a wake-up sound for them and shows your message in a popup")
            .clicked()
        {
            self.open_poke(user);
            ui.close_menu();
        }
        let locally_muted = self.local_muted.lock().unwrap().contains(&user.id);
        let (icon, label) = if locally_muted {
            (icons::regular::SPEAKER_HIGH, "Unmute for Me")
        } else {
            (icons::regular::SPEAKER_SLASH, "Mute for Me")
        };
        if theme::menu_item(ui, icon, label, false).clicked() {
            let mut set = self.local_muted.lock().unwrap();
            if locally_muted {
                set.remove(&user.id);
            } else {
                set.insert(user.id);
            }
            ui.close_menu();
        }
        theme::menu_separator(ui);
        ui.label(format!("{}  User Volume", icons::regular::SPEAKER_HIGH));
        let mut pct = (self.user_volume(user.id) * 100.0).round();
        let slider = egui::Slider::new(&mut pct, 0.0..=crate::audio::MAX_VOLUME * 100.0)
            .suffix(" %")
            .step_by(5.0);
        if ui.add(slider).changed() {
            self.set_user_volume(user.id, &user.name, pct / 100.0);
        }
        if (pct - 100.0).abs() > 0.5 && ui.small_button("Reset to 100 %").clicked() {
            self.set_user_volume(user.id, &user.name, 1.0);
        }
        self.moderation_menu(ui, user);
    }

    /// "Voice Connected" panel above the user panel; in the lobby it explains
    /// that talking and sharing are disabled there.
    fn ui_voice_panel(
        &mut self,
        ui: &mut egui::Ui,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        // Lobby: no voice, so no voice panel (server disconnect sits on the Lobby row).
        let Some(my_ch) = self.my_channel() else { return };
        if self.in_lobby() {
            return;
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    egui::RichText::new(format!("{}  Voice Connected", icons::regular::CELL_SIGNAL_FULL))
                        .strong()
                        .color(theme::GREEN),
                );
                ui.label(egui::RichText::new(self.channel_name(my_ch)).size(12.0).color(theme::MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icons::regular::PHONE_DISCONNECT, IconState::Danger, "Leave voice (back to the lobby)").clicked() {
                    if let Some(spawn) = self.spawn_channel_id() {
                        let _ = cmd_tx.send(Command::JoinChannel(spawn));
                    }
                }
            });
        });

        let cam_live = self.cam.as_ref().is_some_and(|c| c.is_sending());
        let screen_live = self.screen.is_some();
        ui.columns(2, |cols| {
            let (cam_state, cam_tip) = if cam_live {
                (IconState::Active, "Turn off camera")
            } else {
                (IconState::Normal, "Turn on camera (with preview)")
            };
            if wide_toggle(&mut cols[0], icons::regular::VIDEO_CAMERA, "Camera", cam_state, cam_tip).clicked() {
                self.toggle_cam(cmd_tx);
            }
            let (scr_state, scr_tip) = if screen_live {
                (IconState::Active, "Stop sharing")
            } else {
                (IconState::Normal, "Share a screen or window")
            };
            if wide_toggle(&mut cols[1], icons::regular::SCREENCAST, "Screen", scr_state, scr_tip).clicked() {
                self.toggle_screen(cmd_tx);
            }
        });
        ui.add_space(2.0);
        ui.separator();
    }

    /// Bottom user panel: avatar with status dot, name, status; mic and
    /// headphones (each with a device/volume dropdown), settings.
    fn ui_user_panel(
        &mut self,
        ui: &mut egui::Ui,
        cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ) {
        let in_lobby = self.in_lobby();
        let transmitting = self.control.transmitting.load(Ordering::Relaxed) && !in_lobby;
        ui.horizontal(|ui| {
            let av = theme::draw_avatar(ui, &self.config.username, 32.0, transmitting);
            status_dot(ui, av);
            let mode = if in_lobby {
                "Talking disabled in the lobby".to_string()
            } else if self.config.voice_activation {
                "Input: voice activity".to_string()
            } else {
                format!("Input: push to talk ({})", self.config.ptt_key)
            };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(1.0);
                // Leave room for the two split buttons and the gear on the right.
                let max_w = (ui.available_width() - 136.0).max(36.0);
                let name = elide(ui, &self.config.username, &egui::FontId::proportional(14.0), max_w);
                ui.label(egui::RichText::new(name).strong().color(theme::HEADER));
                ui.label(egui::RichText::new("Online").size(11.5).color(theme::MUTED));
            })
            .response
            .on_hover_text(mode);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if theme::icon_button(ui, icons::regular::GEAR_SIX, IconState::Normal, "User settings").clicked() {
                    self.open_settings(SettingsPage::Voice);
                }
                let muted = self.control.self_muted.load(Ordering::Relaxed);
                let deafened = self.control.self_deafened.load(Ordering::Relaxed);
                let deaf_icon = if deafened { icons::regular::SPEAKER_SLASH } else { icons::regular::HEADPHONES };
                let deaf_state = if deafened { IconState::Danger } else { IconState::Normal };
                let (main, caret) =
                    split_button(ui, deaf_icon, deaf_state, if deafened { "Undeafen" } else { "Deafen" }, "Output options");
                if main.clicked() {
                    self.toggle_deafen(cmd_tx);
                }
                self.output_popup(ui, &caret);
                let (mic_icon, mic_state, mic_tip) = if in_lobby {
                    (icons::regular::MICROPHONE_SLASH, IconState::Disabled, "Talking is disabled in the lobby")
                } else if muted {
                    (icons::regular::MICROPHONE_SLASH, IconState::Danger, "Unmute")
                } else {
                    (icons::regular::MICROPHONE, IconState::Normal, "Mute")
                };
                let (main, caret) = split_button(ui, mic_icon, mic_state, mic_tip, "Input options");
                if main.clicked() {
                    self.toggle_mute(cmd_tx);
                }
                self.input_popup(ui, &caret);
            });
        });
    }

    fn input_popup(&mut self, ui: &mut egui::Ui, caret: &egui::Response) {
        let id = ui.make_persistent_id("input_popup");
        if caret.clicked() {
            self.input_devices = capture::input_device_names();
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        let mut save = false;
        egui::popup::popup_above_or_below_widget(
            ui,
            id,
            caret,
            egui::AboveOrBelow::Above,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_width(240.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                theme::caption(ui, "Input device");
                let before = (self.config.input_device.clone(), self.config.output_device.clone());
                device_list(ui, &mut self.config.input_device, &self.input_devices);
                if before.0 != self.config.input_device {
                    self.apply_devices(before);
                }
                theme::menu_separator(ui);
                theme::caption(ui, "Input volume");
                let mut pct = (self.config.input_volume * 100.0).round();
                let r = ui.add(egui::Slider::new(&mut pct, 0.0..=200.0).step_by(5.0).fixed_decimals(0).suffix(" %"));
                if r.changed() {
                    self.config.input_volume = pct / 100.0;
                    self.control.set_input_volume(self.config.input_volume);
                }
                save = r.drag_stopped() || (r.changed() && !r.dragged());
                theme::menu_separator(ui);
                if theme::menu_item(ui, icons::regular::GEAR_SIX, "Voice Settings", false).clicked() {
                    self.open_settings(SettingsPage::Voice);
                    ui.memory_mut(|m| m.close_popup());
                }
            },
        );
        if save {
            self.config.save();
        }
    }

    fn output_popup(&mut self, ui: &mut egui::Ui, caret: &egui::Response) {
        let id = ui.make_persistent_id("output_popup");
        if caret.clicked() {
            self.output_devices = playback::output_device_names();
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        let mut save = false;
        egui::popup::popup_above_or_below_widget(
            ui,
            id,
            caret,
            egui::AboveOrBelow::Above,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_width(240.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                theme::caption(ui, "Output device");
                let before = (self.config.input_device.clone(), self.config.output_device.clone());
                device_list(ui, &mut self.config.output_device, &self.output_devices);
                if before.1 != self.config.output_device {
                    self.apply_devices(before);
                }
                theme::menu_separator(ui);
                theme::caption(ui, "Output volume");
                let mut pct = (self.config.output_volume * 100.0).round();
                let r = ui.add(egui::Slider::new(&mut pct, 0.0..=200.0).step_by(5.0).fixed_decimals(0).suffix(" %"));
                if r.changed() {
                    self.config.output_volume = pct / 100.0;
                    self.control.set_output_volume(self.config.output_volume);
                }
                save = r.drag_stopped() || (r.changed() && !r.dragged());
                theme::menu_separator(ui);
                if theme::menu_item(ui, icons::regular::GEAR_SIX, "Voice Settings", false).clicked() {
                    self.open_settings(SettingsPage::Voice);
                    ui.memory_mut(|m| m.close_popup());
                }
            },
        );
        if save {
            self.config.save();
        }
    }

    pub(super) fn toggle_mute(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        if self.control.self_deafened.load(Ordering::Relaxed) {
            self.set_status("Undeafen before unmuting");
            return;
        }
        let muted = !self.control.self_muted.load(Ordering::Relaxed);
        let deafened = self.control.self_deafened.load(Ordering::Relaxed);
        self.control.self_muted.store(muted, Ordering::Relaxed);
        let _ = cmd_tx.send(Command::SetMute { muted, deafened });
        self.notify(if muted { crate::audio::sfx::Sound::Mute } else { crate::audio::sfx::Sound::Unmute });
    }

    pub(super) fn toggle_deafen(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
        let new = !self.control.self_deafened.load(Ordering::Relaxed);
        if new {
            self.mute_before_deafen = self.control.self_muted.load(Ordering::Relaxed);
            self.control.self_muted.store(true, Ordering::Relaxed);
        } else {
            self.control.self_muted.store(self.mute_before_deafen, Ordering::Relaxed);
        }
        self.control.self_deafened.store(new, Ordering::Relaxed);
        let muted = self.control.self_muted.load(Ordering::Relaxed);
        let _ = cmd_tx.send(Command::SetMute { muted, deafened: new });
        self.notify(if new { crate::audio::sfx::Sound::Mute } else { crate::audio::sfx::Sound::Unmute });
    }
}

/// Icon per channel kind (text = speech bubble, voice = speaker).
pub(super) fn channel_icon(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Spawn => icons::regular::HOUSE,
        ChannelKind::Text => icons::regular::CHAT_CIRCLE_TEXT,
        ChannelKind::Voice => icons::regular::SPEAKER_HIGH,
        ChannelKind::Category => icons::regular::FOLDER,
    }
}

/// Red "disconnect from server" icon at a fixed rect (Lobby row).
fn disconnect_icon(ui: &mut egui::Ui, rect: egui::Rect, tip: &str) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with("lobby_disconnect"), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme::RED.gamma_multiply(0.18));
    }
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icons::regular::SIGN_OUT, egui::FontId::proportional(15.0), theme::RED);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip)
}

/// Small borderless icon button placed at a fixed rect (row hover actions).
fn small_icon(ui: &mut egui::Ui, rect: egui::Rect, icon: &str, tip: &str) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("small_icon", rect.min.x as i32, rect.min.y as i32, icon)), egui::Sense::click());
    let color = if resp.hovered() { theme::HEADER } else { theme::MUTED };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(15.0), color);
    resp.on_hover_text(tip)
}

/// Full-width icon+label toggle (voice panel).
fn wide_toggle(ui: &mut egui::Ui, icon: &str, label: &str, state: IconState, tip: &str) -> egui::Response {
    let (fg, bg) = match state {
        IconState::Active => (egui::Color32::WHITE, theme::GREEN),
        IconState::Disabled => (theme::DISABLED, theme::SIDEBAR),
        _ => (theme::HEADER, theme::INPUT),
    };
    let btn = egui::Button::new(egui::RichText::new(format!("{icon}  {label}")).color(fg))
        .fill(bg)
        .min_size(egui::vec2(ui.available_width(), 32.0));
    ui.add_enabled(state != IconState::Disabled, btn)
        .on_hover_text(tip)
        .on_disabled_hover_text(tip)
}

/// Green "online" dot at the bottom right of the own avatar.
fn status_dot(ui: &egui::Ui, avatar: egui::Rect) {
    let r = 5.0;
    let c = avatar.right_bottom() - egui::vec2(r * 0.7, r * 0.7);
    ui.painter().circle_filled(c, r + 2.5, theme::PANEL);
    ui.painter().circle_filled(c, r, theme::GREEN);
}

/// Icon button with a dropdown caret on its right, drawn as one pill.
/// Returns (icon part, caret part).
fn split_button(
    ui: &mut egui::Ui,
    icon: &str,
    state: IconState,
    tip: &str,
    caret_tip: &str,
) -> (egui::Response, egui::Response) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 32.0), egui::Sense::hover());
    let (main_r, caret_r) = rect.split_left_right_at_x(rect.left() + 30.0);
    let enabled = state != IconState::Disabled;
    let main = ui.interact(main_r, ui.id().with(("split_main", icon, tip)), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let caret = ui.interact(caret_r, ui.id().with(("split_caret", caret_tip)), egui::Sense::click());
    let p = ui.painter();
    let fg = match state {
        IconState::Normal => theme::TEXT,
        IconState::Active => theme::GREEN,
        IconState::Danger => theme::RED,
        IconState::Disabled => theme::DISABLED,
    };
    if state == IconState::Danger {
        p.rect_filled(rect, 6.0, theme::RED.gamma_multiply(0.15));
    }
    if main.hovered() && enabled {
        p.rect_filled(main_r, egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, egui::Color32::from_white_alpha(14));
    }
    if caret.hovered() {
        p.rect_filled(caret_r, egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, egui::Color32::from_white_alpha(14));
    }
    p.text(main_r.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(18.0), fg);
    p.text(caret_r.center(), egui::Align2::CENTER_CENTER, icons::regular::CARET_DOWN, egui::FontId::proportional(12.0), theme::MUTED);
    (main.on_hover_text(tip), caret.on_hover_text(caret_tip))
}

/// Selectable device rows ("System default" first).
fn device_list(ui: &mut egui::Ui, selected: &mut Option<String>, devices: &[String]) {
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        if ui.selectable_label(selected.is_none(), "System default").clicked() {
            *selected = None;
        }
        for name in devices {
            if ui.selectable_label(selected.as_deref() == Some(name), name).clicked() {
                *selected = Some(name.clone());
            }
        }
    });
}

/// Cut text with "..." to fit `max_w`.
pub(super) fn elide(ui: &egui::Ui, text: &str, font: &egui::FontId, max_w: f32) -> String {
    let width = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>() + "...";
        if width(&s) <= max_w {
            return s;
        }
    }
    "...".to_string()
}

/// The pointer is over `rect` (hovering or pressed there), and no popup or
/// window lies on top of it.
fn pointer_on(ui: &egui::Ui, rect: egui::Rect) -> bool {
    let ctx = ui.ctx();
    let Some(p) = ctx.pointer_hover_pos().or_else(|| ctx.pointer_interact_pos()) else { return false };
    rect.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l == ui.layer_id())
}
