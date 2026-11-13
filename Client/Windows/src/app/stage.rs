//! Local layouts of the voice stage: grid, focus (one tile large) and
//! presenter (a screen with the same user's camera on top). Only this
//! client's view changes; nothing about it goes to the server.

use super::media_ui::kind_icon;
use super::theme;
use super::views::{grid_rects, member_card, name_pill, stage_grid, video_tile, watch_tile};
use super::*;
use crate::config::StageMode;

/// One stage tile: a user's screen, camera, or (`None`) their avatar card.
pub(super) type TileKey = (u64, Option<VideoKind>);

/// What the user picked for Focus / Presenter; `None` = automatic.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct StagePick {
    pub focus: Option<TileKey>,
    pub presenter: Option<u64>,
}

const ASPECT: f32 = 16.0 / 9.0;
const GAP: f32 = 12.0;
/// Tiles beside the focused one are at least this fraction of its size.
const STRIP: f32 = 0.25;
/// Height kept free under the tiles for the layout buttons.
const CONTROLS_BAND: f32 = 50.0;
/// Camera overlay width relative to the presented screen.
const OVERLAY: f32 = 0.25;

impl StageMode {
    fn hint(self) -> &'static str {
        match self {
            StageMode::Grid => "All tiles the same size",
            StageMode::Focus => "One stream or person large, the others beside or below",
            StageMode::Presenter => "A screen share large with the same person's camera on top",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            StageMode::Grid => "Grid",
            StageMode::Focus => "Focus",
            StageMode::Presenter => "Presenter",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            StageMode::Grid => icons::regular::SQUARES_FOUR,
            StageMode::Focus => icons::regular::LAYOUT,
            StageMode::Presenter => icons::regular::PICTURE_IN_PICTURE,
        }
    }
}

/// Why a user can't be shown as presenter (`None` = they can).
pub(super) fn presenter_blocker(streams: &[VideoKind]) -> Option<&'static str> {
    match (
        streams.contains(&VideoKind::Screen),
        streams.contains(&VideoKind::Camera),
    ) {
        (true, true) => None,
        (false, false) => Some("no screen or camera"),
        (false, true) => Some("no screen share"),
        (true, false) => Some("camera is off"),
    }
}

/// Index of the focused tile: the pick if it is still there, else another
/// tile of the same person (camera turned on or off), else the first tile
/// (screen shares come first).
pub(super) fn resolve_focus(tiles: &[TileKey], pick: Option<TileKey>) -> Option<usize> {
    if tiles.is_empty() {
        return None;
    }
    pick.and_then(|p| {
        tiles
            .iter()
            .position(|t| *t == p)
            .or_else(|| tiles.iter().position(|t| t.0 == p.0))
    })
    .or(Some(0))
}

/// What the Presenter layout shows.
#[derive(Debug, PartialEq)]
pub(super) enum Presenting {
    /// Screen large, camera overlay.
    Both(u64),
    /// The picked user turned the camera off: their screen alone, large.
    ScreenOnly(u64),
    /// Nobody can present: plain grid.
    Nobody,
}

/// `candidates`: (user id, streams) of everyone in the channel, in stage order.
pub(super) fn resolve_presenter(
    candidates: &[(u64, Vec<VideoKind>)],
    pick: Option<u64>,
) -> Presenting {
    let picked = pick.and_then(|p| candidates.iter().find(|(id, _)| *id == p));
    if let Some((id, streams)) = picked {
        if presenter_blocker(streams).is_none() {
            return Presenting::Both(*id);
        }
        if streams.contains(&VideoKind::Screen) {
            return Presenting::ScreenOnly(*id);
        }
    }
    match candidates
        .iter()
        .find(|(_, s)| presenter_blocker(s).is_none())
    {
        Some((id, _)) => Presenting::Both(*id),
        None => Presenting::Nobody,
    }
}

/// Focus layout: the big 16:9 rect plus `n_others` tiles in a strip to the
/// right or below it, whichever leaves the big one larger.
pub(super) fn focus_rects(
    n_others: usize,
    area: egui::Rect,
    gap: f32,
) -> (egui::Rect, Vec<egui::Rect>) {
    let (w, h) = (area.width(), area.height());
    if n_others == 0 {
        let (_, tile) = stage_grid(1, area.size(), gap);
        let main = grid_rects(1, 1, tile, gap, area)
            .first()
            .copied()
            .unwrap_or(area);
        return (main, Vec::new());
    }
    let right = ((w - gap) / (1.0 + STRIP)).min(h * ASPECT).max(0.0);
    let below = ((h - gap) / (1.0 + STRIP)).min(w / ASPECT).max(0.0) * ASPECT;
    let (main, strip) = if right >= below {
        let size = egui::vec2(right, right / ASPECT);
        let strip_w = (w - right - gap).clamp(0.0, right * 0.5);
        let left = area.left() + ((w - right - gap - strip_w) / 2.0).max(0.0);
        let main =
            egui::Rect::from_min_size(egui::pos2(left, area.top() + (h - size.y) / 2.0), size);
        let strip = egui::Rect::from_min_size(
            egui::pos2(main.right() + gap, area.top()),
            egui::vec2(strip_w, h),
        );
        (main, strip)
    } else {
        let size = egui::vec2(below, below / ASPECT);
        let strip_h = (h - size.y - gap).clamp(0.0, size.y * 0.5);
        let top = area.top() + ((h - size.y - gap - strip_h) / 2.0).max(0.0);
        let main =
            egui::Rect::from_min_size(egui::pos2(area.left() + (w - size.x) / 2.0, top), size);
        let strip = egui::Rect::from_min_size(
            egui::pos2(area.left(), main.bottom() + gap),
            egui::vec2(w, strip_h),
        );
        (main, strip)
    };
    let (cols, tile) = stage_grid(n_others, strip.size(), gap);
    (main, grid_rects(n_others, cols, tile, gap, strip))
}

/// Camera overlay in the bottom left of the presented picture.
pub(super) fn overlay_rect(content: egui::Rect, cam_aspect: f32) -> egui::Rect {
    let aspect = if cam_aspect.is_finite() && cam_aspect > 0.2 {
        cam_aspect
    } else {
        ASPECT
    };
    let mut w = (content.width() * OVERLAY).max(96.0f32.min(content.width() * 0.4));
    if w / aspect > content.height() * 0.45 {
        w = content.height() * 0.45 * aspect;
    }
    let margin = (content.width() * 0.02).clamp(6.0, 16.0);
    let size = egui::vec2(w, w / aspect);
    let top = (content.bottom() - margin - size.y).max(content.top());
    egui::Rect::from_min_size(egui::pos2(content.left() + margin, top), size)
}

fn tile_label(user: &UserInfo, kind: Option<VideoKind>) -> String {
    match kind {
        Some(VideoKind::Screen) => format!("{} - screen", user.name),
        Some(VideoKind::Camera) => format!("{} - camera", user.name),
        None => user.name.clone(),
    }
}

impl RustTalkApp {
    pub(super) fn channel_members(&self, ch_id: u64) -> Vec<UserInfo> {
        self.srv
            .channels
            .get(&ch_id)
            .map(|c| c.user_ids.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| self.srv.users.get(&id).cloned())
            .collect()
    }

    /// Stage tiles in order: screen shares first, then everyone else (their
    /// camera instead of the avatar card when it is on).
    pub(super) fn stage_tiles(&self, members: &[UserInfo]) -> Vec<(UserInfo, Option<VideoKind>)> {
        let mut tiles = Vec::new();
        for user in members {
            if self.streams_of(user).contains(&VideoKind::Screen) {
                tiles.push((user.clone(), Some(VideoKind::Screen)));
            }
        }
        for user in members {
            let cam = self.streams_of(user).contains(&VideoKind::Camera);
            tiles.push((user.clone(), cam.then_some(VideoKind::Camera)));
        }
        tiles
    }

    /// The presenter whose screen `key` is, while the Presenter layout shows
    /// them with their camera (fullscreen keeps the camera on top).
    pub(super) fn presenter_of(&self, key: (u64, VideoKind)) -> Option<UserInfo> {
        if self.config.stage_mode != StageMode::Presenter || key.1 != VideoKind::Screen {
            return None;
        }
        let user = self.srv.users.get(&key.0)?;
        let candidates: Vec<(u64, Vec<VideoKind>)> = self
            .channel_members(user.channel_id)
            .iter()
            .map(|u| (u.id, self.streams_of(u)))
            .collect();
        (resolve_presenter(&candidates, self.stage_pick.presenter) == Presenting::Both(key.0))
            .then(|| user.clone())
    }

    /// Picking a target is an explicit "show me this": start watching it.
    fn pick_stage_target(&mut self, mode: StageMode, user: u64, kind: Option<VideoKind>) {
        // Ids of another server would name someone else on the voice server.
        if !self.viewing_voice() {
            return;
        }
        match mode {
            StageMode::Focus => {
                self.stage_pick.focus = Some((user, kind));
                if let Some(kind) = kind {
                    self.watch(user, kind);
                }
            }
            StageMode::Presenter => {
                self.stage_pick.presenter = Some(user);
                self.watch(user, VideoKind::Screen);
                self.watch(user, VideoKind::Camera);
            }
            StageMode::Grid => {}
        }
    }

    /// Layout buttons (Grid / Focus / Presenter) in the bottom right corner of the stage and,
    /// for Focus / Presenter, a menu with who is shown.
    pub(super) fn stage_controls(&mut self, ui: &mut egui::Ui, area: egui::Rect, ch_id: u64) {
        let mode = self.config.stage_mode;
        let count = if mode == StageMode::Grid { 3.0 } else { 4.0 };
        let size = egui::vec2(count * 34.0 + 8.0, 40.0);
        let pill = egui::Rect::from_min_size(
            area.right_bottom() - size - egui::vec2(10.0, 10.0),
            size,
        );
        ui.painter()
            .rect_filled(pill, 10.0, egui::Color32::from_black_alpha(170));
        let mut picked = None;
        let mut target = None;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(pill.shrink(4.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for m in [StageMode::Grid, StageMode::Focus, StageMode::Presenter] {
                    let state = if m == mode {
                        theme::IconState::Active
                    } else {
                        theme::IconState::Normal
                    };
                    let tip = format!("{}: {} (only on your screen)", m.label(), m.hint());
                    if theme::icon_button(ui, m.icon(), state, &tip).clicked() {
                        picked = Some(m);
                    }
                }
                if mode != StageMode::Grid {
                    target = Some(theme::icon_button(
                        ui,
                        icons::regular::USER_FOCUS,
                        theme::IconState::Normal,
                        &format!("Who {} shows (only on your screen)", mode.label()),
                    ));
                }
            },
        );
        if let Some(m) = picked {
            if m != mode {
                self.config.stage_mode = m;
                self.config.save();
            }
        }
        let Some(resp) = target else { return };
        let popup_id = ui.make_persistent_id("stage_target_menu");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup_id));
        }
        let members = self.channel_members(ch_id);
        let tiles = self.stage_tiles(&members);
        let candidates: Vec<(u64, Vec<VideoKind>)> =
            members.iter().map(|u| (u.id, self.streams_of(u))).collect();
        let mut toggle_cutout = false;
        let mut chosen = None;
        egui::popup::popup_above_or_below_widget(
            ui,
            popup_id,
            &resp,
            egui::AboveOrBelow::Above,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(300.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    egui::RichText::new(format!("{} on", mode.label()))
                        .strong()
                        .color(theme::HEADER),
                );
                theme::menu_separator(ui);
                if members.is_empty() {
                    ui.label(egui::RichText::new("Nobody is in this channel").color(theme::MUTED));
                }
                match mode {
                    StageMode::Focus => {
                        for (user, kind) in &tiles {
                            let selected = self.stage_pick.focus == Some((user.id, *kind));
                            let icon = if selected {
                                icons::regular::CHECK
                            } else {
                                kind.map(kind_icon).unwrap_or(icons::regular::USER)
                            };
                            if theme::menu_item(ui, icon, &tile_label(user, *kind), false).clicked()
                            {
                                chosen = Some((user.id, *kind));
                            }
                        }
                    }
                    _ => {
                        for (user, (_, streams)) in members.iter().zip(&candidates) {
                            let blocker = presenter_blocker(streams);
                            let selected = self.stage_pick.presenter == Some(user.id);
                            let icon = if selected {
                                icons::regular::CHECK
                            } else {
                                icons::regular::PRESENTATION
                            };
                            let label = match blocker {
                                Some(why) => format!("{} - {why}", user.name),
                                None => user.name.clone(),
                            };
                            let item = ui
                                .add_enabled_ui(blocker.is_none(), |ui| {
                                    theme::menu_item(ui, icon, &label, false)
                                })
                                .inner;
                            if item.clicked() {
                                chosen = Some((user.id, None));
                            }
                            if let Some(why) = blocker {
                                item.on_disabled_hover_text(format!(
                                    "Presenter needs a screen or game stream and a camera: {why}"
                                ));
                            }
                        }
                    }
                }
                theme::menu_separator(ui);
                let auto = match mode {
                    StageMode::Focus => self.stage_pick.focus.is_none(),
                    _ => self.stage_pick.presenter.is_none(),
                };
                if theme::menu_item(
                    ui,
                    if auto {
                        icons::regular::CHECK
                    } else {
                        icons::regular::MAGIC_WAND
                    },
                    "Automatic",
                    false,
                )
                .clicked()
                {
                    self.stage_pick = match mode {
                        StageMode::Focus => StagePick {
                            focus: None,
                            ..self.stage_pick.clone()
                        },
                        _ => StagePick {
                            presenter: None,
                            ..self.stage_pick.clone()
                        },
                    };
                    ui.memory_mut(|m| m.close_popup());
                }
                if mode == StageMode::Presenter {
                    theme::menu_separator(ui);
                    let on = self.config.remove_background;
                    let icon = if on {
                        icons::regular::CHECK_SQUARE
                    } else {
                        icons::regular::SQUARE
                    };
                    if theme::menu_item(ui, icon, "Remove background", false)
                        .on_hover_text(
                            "Cuts the presenter out of their camera picture, on this computer only",
                        )
                        .clicked()
                    {
                        toggle_cutout = true;
                    }
                }
            },
        );
        if toggle_cutout {
            self.config.remove_background = !self.config.remove_background;
            self.config.save();
        }
        if let Some((user, kind)) = chosen {
            self.pick_stage_target(mode, user, kind);
            ui.memory_mut(|m| m.close_popup());
        }
    }

    /// Lays out and draws the stage tiles in the chosen layout.
    pub(super) fn ui_stage_tiles(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        tiles: &[(UserInfo, Option<VideoKind>)],
        speaking_of: &dyn Fn(&UserInfo) -> bool,
    ) {
        // The layout buttons get their own band under the tiles.
        let full = area;
        let area = if tiles.len() > 1 {
            egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, (area.max.y - CONTROLS_BAND).max(area.min.y)))
        } else {
            area
        };
        let keys: Vec<TileKey> = tiles.iter().map(|(u, k)| (u.id, *k)).collect();
        let mut placed: Vec<(usize, egui::Rect)> = Vec::new();
        let mut overlay: Option<(egui::Rect, usize)> = None;
        let mut hint = None;
        let big = |main: usize, skip: &[usize]| {
            let others: Vec<usize> = (0..tiles.len())
                .filter(|i| *i != main && !skip.contains(i))
                .collect();
            let (main_rect, rects) = focus_rects(others.len(), area, GAP);
            let mut out = vec![(main, main_rect)];
            out.extend(others.into_iter().zip(rects));
            out
        };
        match self.config.stage_mode {
            StageMode::Focus => {
                if let Some(i) = resolve_focus(&keys, self.stage_pick.focus) {
                    placed = big(i, &[]);
                }
            }
            StageMode::Presenter => {
                let candidates: Vec<(u64, Vec<VideoKind>)> = {
                    let mut seen = Vec::new();
                    for (u, _) in tiles {
                        if !seen.iter().any(|(id, _)| *id == u.id) {
                            seen.push((u.id, self.streams_of(u)));
                        }
                    }
                    seen
                };
                let find = |id: u64, k: VideoKind| keys.iter().position(|t| *t == (id, Some(k)));
                match resolve_presenter(&candidates, self.stage_pick.presenter) {
                    Presenting::Both(id) => {
                        if let (Some(screen), Some(cam)) =
                            (find(id, VideoKind::Screen), find(id, VideoKind::Camera))
                        {
                            placed = big(screen, &[cam]);
                            overlay = Some((placed[0].1, cam));
                        }
                    }
                    Presenting::ScreenOnly(id) => {
                        if let Some(screen) = find(id, VideoKind::Screen) {
                            placed = big(screen, &[]);
                        }
                    }
                    Presenting::Nobody => {
                        hint = Some("Presenter: nobody shares a screen and a camera")
                    }
                }
            }
            StageMode::Grid => {}
        }
        let main_tile = placed.first().map(|(i, _)| *i);
        if placed.is_empty() {
            let (cols, tile) = stage_grid(tiles.len(), area.size(), GAP);
            placed = grid_rects(tiles.len(), cols, tile, GAP, area)
                .into_iter()
                .enumerate()
                .collect();
        }

        let mut open_fs = None;
        let mut focus_click = None;
        for (i, rect) in placed.iter().copied() {
            let (user, kind) = &tiles[i];
            let show_name = overlay.is_none_or(|(_, cam)| tiles[cam].0.id != user.id);
            self.stage_tile(
                ui,
                rect,
                user,
                *kind,
                speaking_of(user),
                show_name,
                &mut open_fs,
                main_tile == Some(i),
                &mut focus_click,
            );
        }
        if let Some((main, cam)) = overlay {
            // The main tile is 16:9: the camera sits in its corner, also when
            // a 21:9 share leaves bars there.
            let user = &tiles[cam].0;
            self.presenter_overlay(ui, main, user, speaking_of(user), &mut open_fs);
        }
        if let Some(hint) = hint {
            let p = ui.painter();
            let galley =
                p.layout_no_wrap(hint.into(), egui::FontId::proportional(13.0), theme::HEADER);
            let pill = egui::Rect::from_center_size(
                egui::pos2(area.center().x, area.top() + galley.size().y / 2.0 + 4.0),
                galley.size() + egui::vec2(20.0, 8.0),
            );
            p.rect_filled(pill, 8.0, egui::Color32::from_black_alpha(190));
            p.galley(pill.center() - galley.size() / 2.0, galley, theme::HEADER);
        }
        if let Some((user, kind)) = focus_click {
            self.config.stage_mode = StageMode::Focus;
            self.config.save();
            self.pick_stage_target(StageMode::Focus, user, kind);
        }
        if let Some(key) = open_fs {
            self.enter_fullscreen(key);
        }
        if tiles.len() > 1 {
            self.stage_controls(ui, full, tiles.first().map_or(0, |(u, _)| u.channel_id));
        }
    }

    /// One tile: "Watch" card, video or avatar card, plus the user menu.
    #[allow(clippy::too_many_arguments)]
    fn stage_tile(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        user: &UserInfo,
        kind: Option<VideoKind>,
        speaking: bool,
        show_name: bool,
        open_fs: &mut Option<(u64, VideoKind)>,
        main: bool,
        focus_click: &mut Option<(u64, Option<VideoKind>)>,
    ) {
        let me = user.id == self.srv.my_id;
        // Speaking ring only on the person's camera, not on screen shares.
        let cam_speaking = kind == Some(VideoKind::Camera) && speaking;
        let resp = match kind {
            Some(kind) if !me && !self.watching.contains(&(user.id, kind)) => {
                let resp = watch_tile(ui, rect, user, kind, cam_speaking);
                if resp.clicked() {
                    if main {
                        self.watch(user.id, kind);
                    } else {
                        *focus_click = Some((user.id, Some(kind)));
                    }
                }
                resp
            }
            Some(kind) => {
                let tex = self
                    .video_frames
                    .get(&(user.id, kind))
                    .and_then(|v| v.tex.clone());
                let resp = video_tile(
                    ui,
                    rect,
                    tex.as_ref(),
                    user,
                    kind,
                    cam_speaking,
                    me,
                    show_name,
                );
                if resp.clicked() {
                    if main {
                        *open_fs = Some((user.id, kind));
                    } else {
                        *focus_click = Some((user.id, Some(kind)));
                    }
                }
                // Watching someone else: "stop watching" in the top right corner.
                if !me && (resp.hovered() || ui.rect_contains_pointer(rect)) {
                    let b = egui::Rect::from_center_size(
                        rect.right_top() + egui::vec2(-22.0, 22.0),
                        egui::vec2(30.0, 30.0),
                    );
                    let stop = ui
                        .put(
                            b,
                            egui::Button::new(
                                egui::RichText::new(icons::regular::EYE_SLASH)
                                    .size(16.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_black_alpha(160))
                            .corner_radius(15.0),
                        )
                        .on_hover_text("Stop watching");
                    if stop.clicked() {
                        self.unwatch(user.id, kind);
                    }
                }
                resp
            }
            None => {
                let resp = member_card(ui, rect, user, speaking, me);
                if resp.clicked() && !main {
                    *focus_click = Some((user.id, None));
                }
                resp
            }
        };
        if !main && ui.rect_contains_pointer(rect) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if user.audio_sharing && kind != Some(VideoKind::Screen) {
            let lit = self.shared_audio_lit(user);
            let badge = egui::Rect::from_center_size(rect.left_top() + egui::vec2(24.0, 24.0), egui::vec2(28.0, 28.0));
            let p = ui.painter();
            p.circle_filled(badge.center(), 14.0, if lit { theme::GREEN } else { egui::Color32::from_black_alpha(160) });
            p.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                icons::regular::MUSIC_NOTES,
                egui::FontId::proportional(16.0),
                if lit { egui::Color32::WHITE } else { theme::MUTED },
            );
            ui.interact(badge, ui.id().with(("audio_badge", user.id, kind)), egui::Sense::hover())
                .on_hover_text(if lit { "Sharing audio (sound playing)" } else { "Sharing audio" });
        }
        // Right click / "..." button: same menu as in the sidebar.
        if !me {
            resp.context_menu(|ui| self.user_menu(ui, user));
            self.tile_menu_button(ui, rect, resp.hovered(), user, kind);
        }
    }

    /// The presenter's camera, small in the bottom left of their screen.
    pub(super) fn presenter_overlay(
        &mut self,
        ui: &mut egui::Ui,
        content: egui::Rect,
        user: &UserInfo,
        speaking: bool,
        open_fs: &mut Option<(u64, VideoKind)>,
    ) {
        let key = (user.id, VideoKind::Camera);
        let watched = user.id == self.srv.my_id || self.watching.contains(&key);
        let tex = if watched {
            self.video_frames.get(&key).and_then(|v| v.tex.clone())
        } else {
            None
        };
        let aspect = tex
            .as_ref()
            .map(|t| t.size_vec2())
            .map_or(ASPECT, |s| s.x / s.y);
        let rect = overlay_rect(content, aspect);
        let resp = ui.interact(
            rect,
            ui.id().with(("presenter_cam", user.id)),
            egui::Sense::click(),
        );
        // Cut out: only the person on top of the screen, no frame around them.
        let cut = watched && self.config.remove_background;
        if cut {
            self.cutout_key = Some(key);
        }
        let p = ui.painter();
        if !(cut && tex.is_some()) {
            p.rect_filled(rect.expand(2.0), 10.0, egui::Color32::from_black_alpha(140));
        }
        match &tex {
            Some(tex) => egui::Image::new(tex).corner_radius(8.0).paint_at(ui, rect),
            None if watched => {
                p.rect_filled(rect, 8.0, egui::Color32::BLACK);
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Connecting...",
                    egui::FontId::proportional(12.0),
                    theme::MUTED,
                );
            }
            None => {
                p.rect_filled(
                    rect,
                    8.0,
                    theme::avatar_color(&user.name).linear_multiply(0.35),
                );
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}  Watch camera", icons::regular::EYE),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
            }
        }
        name_pill(p, rect, user.name.clone());
        if speaking && !cut {
            p.rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(3.0_f32, theme::GREEN),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(2.0_f32, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if watched {
            if resp.on_hover_text("Click for fullscreen").clicked() {
                *open_fs = Some(key);
            }
        } else if resp.on_hover_text("Click to watch the camera").clicked() {
            self.watch(user.id, VideoKind::Camera);
        }
    }
}

/// Debug builds: drawn pictures for `RUSTTALK_DEV_FAKE_VIDEO` (a person in
/// a room, or a slide with a bar chart).
#[cfg(debug_assertions)]
pub(super) fn dev_fake_picture(kind: VideoKind) -> egui::ColorImage {
    if kind == VideoKind::Screen {
        let (w, h) = (1280usize, 720usize);
        let bars = [0.35f32, 0.6, 0.45, 0.8, 0.7];
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let c = if y < 90 {
                    egui::Color32::from_rgb(0x58, 0x65, 0xF2)
                } else if ((160..300).contains(&x) && (130..150).contains(&y))
                    || ((160..700).contains(&x) && (170..185).contains(&y))
                {
                    egui::Color32::from_rgb(0x99, 0xA0, 0xB0)
                } else {
                    let slot = x.saturating_sub(560) / 130;
                    let in_bar = x >= 560 && (x - 560) % 130 < 90 && slot < bars.len();
                    let top = 640.0 - bars.get(slot).copied().unwrap_or(0.0) * 400.0;
                    if in_bar && (y as f32) >= top && y < 640 {
                        egui::Color32::from_rgb(0x3B, 0xA5, 0x5D)
                    } else {
                        egui::Color32::from_rgb(0x1E, 0x1F, 0x22)
                    }
                };
                px.push(c);
            }
        }
        return egui::ColorImage {
            size: [w, h],
            pixels: px,
        };
    }
    let (w, h) = (640usize, 360usize);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let head = ((fx - 320.0) / 52.0).powi(2) + ((fy - 135.0) / 62.0).powi(2) <= 1.0;
            let body = ((fx - 320.0) / 150.0).powi(2) + ((fy - 360.0) / 140.0).powi(2) <= 1.0;
            let c = if head {
                egui::Color32::from_rgb(0xE0, 0xAC, 0x8A)
            } else if body {
                egui::Color32::from_rgb(0x2E, 0x6F, 0xB8)
            } else if (x / 80 + y / 60) % 2 == 0 {
                egui::Color32::from_rgb(0x8A, 0x7F, 0x6C)
            } else {
                egui::Color32::from_rgb(0x9C, 0x91, 0x7C)
            };
            px.push(c);
        }
    }
    egui::ColorImage {
        size: [w, h],
        pixels: px,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, h))
    }

    fn inside(outer: egui::Rect, r: egui::Rect) -> bool {
        outer.expand(0.5).contains_rect(r)
    }

    fn user(id: u64, name: &str) -> UserInfo {
        UserInfo {
            id,
            name: name.into(),
            channel_id: 2,
            muted: false,
            deafened: false,
            sharing: false,
            audio_sharing: false,
            role: Default::default(),
            streams: vec![],
        }
    }

    /// Runs the stage in a 1200x700 screen; returns the stage area it drew into.
    fn run_stage(
        ctx: &egui::Context,
        app: &mut RustTalkApp,
        tiles: &[(UserInfo, Option<VideoKind>)],
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let mut area = egui::Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 700.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    area = ui.max_rect();
                    app.ui_stage_tiles(ui, area, tiles, &|_| false);
                });
            },
        );
        area
    }

    fn click_at(ctx: &egui::Context, app: &mut RustTalkApp, tiles: &[(UserInfo, Option<VideoKind>)], pos: egui::Pos2, t: &mut f64) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [vec![egui::Event::PointerMoved(pos)], vec![button(true)], vec![button(false)]] {
            *t += 0.1;
            run_stage(ctx, app, tiles, *t, events);
        }
    }

    #[test]
    fn stage_buttons_switch_layout_and_a_tile_click_focuses_it() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let tiles: Vec<_> = (1..=3).map(|i| (user(i, &format!("user{i}")), None)).collect();
        app.config.stage_mode = StageMode::Grid;
        let mut t = 0.0;
        let area = run_stage(&ctx, &mut app, &tiles, t, vec![]);
        // Pill: bottom right of the stage, one button per layout.
        let button = |n: f32, count: f32| {
            let size = egui::vec2(count * 34.0 + 8.0, 40.0);
            let pill = egui::Rect::from_min_size(area.right_bottom() - size - egui::vec2(10.0, 10.0), size);
            egui::pos2(pill.left() + 4.0 + 16.0 + 34.0 * n, pill.center().y)
        };
        click_at(&ctx, &mut app, &tiles, button(2.0, 3.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Presenter);
        click_at(&ctx, &mut app, &tiles, button(0.0, 4.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Grid);
        click_at(&ctx, &mut app, &tiles, button(1.0, 3.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Focus);
        // Back in Grid, clicking a person tile focuses it.
        click_at(&ctx, &mut app, &tiles, button(0.0, 4.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Grid);
        let tile_area = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.max.y - CONTROLS_BAND));
        let (cols, tile) = stage_grid(3, tile_area.size(), GAP);
        let first = grid_rects(3, cols, tile, GAP, tile_area)[0];
        assert!(!first.intersects(egui::Rect::from_min_size(
            area.right_bottom() - egui::vec2(160.0, 50.0),
            egui::vec2(160.0, 50.0)
        )), "tiles stay clear of the buttons");
        click_at(&ctx, &mut app, &tiles, first.center(), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Focus);
    }

    #[test]
    fn focus_strip_goes_beside_on_wide_and_below_on_tall_areas() {
        let wide = area(1400.0, 450.0);
        let (main, others) = focus_rects(3, wide, GAP);
        assert!(
            others.iter().all(|r| r.left() >= main.right()),
            "strip right of the focused tile"
        );
        let tall = area(700.0, 800.0);
        let (main, others) = focus_rects(3, tall, GAP);
        assert!(
            others.iter().all(|r| r.top() >= main.bottom()),
            "strip below the focused tile"
        );
    }

    #[test]
    fn focus_tiles_stay_inside_and_never_overlap() {
        for (w, h) in [
            (1200.0, 400.0),
            (800.0, 600.0),
            (500.0, 900.0),
            (300.0, 200.0),
        ] {
            for n in 0..9 {
                let a = area(w, h);
                let (main, others) = focus_rects(n, a, GAP);
                assert_eq!(others.len(), n);
                assert!(inside(a, main), "{w}x{h} n={n}: main {main:?}");
                assert!((main.width() / main.height() - ASPECT).abs() < 0.01);
                for (i, r) in others.iter().enumerate() {
                    assert!(inside(a, *r), "{w}x{h} n={n}: tile {r:?}");
                    assert!(!r.shrink(0.5).intersects(main));
                    assert!(r.width() < main.width());
                    for s in &others[i + 1..] {
                        assert!(!r.shrink(0.5).intersects(*s));
                    }
                }
            }
        }
    }

    #[test]
    fn focus_alone_fills_like_the_grid() {
        let a = area(1200.0, 400.0);
        let (main, _) = focus_rects(0, a, GAP);
        let (_, tile) = stage_grid(1, a.size(), GAP);
        assert!(
            (main.size() - tile).length() < 0.01,
            "{:?} vs {tile:?}",
            main.size()
        );
    }

    #[test]
    fn overlay_is_a_quarter_in_the_bottom_left() {
        let content = area(1600.0, 900.0);
        let r = overlay_rect(content, ASPECT);
        assert!(inside(content, r));
        assert!((r.width() - 400.0).abs() < 0.5);
        assert!(r.left() - content.left() < 20.0 && content.bottom() - r.bottom() < 20.0);
        // Portrait camera on a flat strip: limited by height, still inside.
        let flat = area(1600.0, 300.0);
        let r = overlay_rect(flat, 9.0 / 16.0);
        assert!(inside(flat, r) && r.height() <= 300.0 * 0.45 + 0.5);
        // Broken sizes fall back to 16:9.
        assert!(overlay_rect(content, f32::NAN).width() > 0.0);
    }

    #[test]
    fn focus_keeps_the_pick_and_falls_back_to_the_first_tile() {
        let tiles = [
            (1, Some(VideoKind::Screen)),
            (1, Some(VideoKind::Camera)),
            (2, None),
        ];
        assert_eq!(resolve_focus(&tiles, Some((2, None))), Some(2));
        assert_eq!(resolve_focus(&tiles, Some((9, None))), Some(0));
        // Picked the card, then the camera came on (or the other way round).
        let cam_on = [(1, Some(VideoKind::Screen)), (2, Some(VideoKind::Camera))];
        assert_eq!(resolve_focus(&cam_on, Some((2, None))), Some(1));
        assert_eq!(
            resolve_focus(&tiles, Some((2, Some(VideoKind::Camera)))),
            Some(2)
        );
        assert_eq!(resolve_focus(&tiles, None), Some(0));
        assert_eq!(resolve_focus(&[], None), None);
    }

    #[test]
    fn presenter_needs_screen_and_camera() {
        use VideoKind::*;
        assert_eq!(presenter_blocker(&[Screen, Camera]), None);
        assert_eq!(presenter_blocker(&[Camera]), Some("no screen share"));
        assert_eq!(presenter_blocker(&[Screen]), Some("camera is off"));
        assert!(presenter_blocker(&[]).is_some());

        let cands = vec![
            (1, vec![Camera]),
            (2, vec![Screen, Camera]),
            (3, vec![Screen]),
        ];
        assert_eq!(resolve_presenter(&cands, None), Presenting::Both(2));
        assert_eq!(resolve_presenter(&cands, Some(2)), Presenting::Both(2));
        // Picked user turned the camera off: keep their screen.
        assert_eq!(
            resolve_presenter(&cands, Some(3)),
            Presenting::ScreenOnly(3)
        );
        // Picked user can't present at all: someone who can.
        assert_eq!(resolve_presenter(&cands, Some(1)), Presenting::Both(2));
        assert_eq!(
            resolve_presenter(&[(1, vec![Screen])], None),
            Presenting::Nobody
        );
    }

    #[test]
    fn modes_persist_by_name() {
        let cfg: crate::config::ClientConfig =
            toml::from_str("stage_mode = \"presenter\"").unwrap();
        assert_eq!(cfg.stage_mode, StageMode::Presenter);
        let cfg: crate::config::ClientConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.stage_mode, StageMode::Grid);
    }
}
