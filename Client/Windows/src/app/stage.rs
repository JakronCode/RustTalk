//! Local layouts of the voice stage: grid, focus (one tile large) and
//! presenter (a screen with the same user's camera on top). Only this
//! client's view changes; nothing about it goes to the server.

use super::media_ui::kind_icon;
use super::theme;
use super::views::{grid_rects, member_card, name_pill, stage_grid, video_tile, watch_tile};
use super::*;
use crate::config::{CamCorner, StageMode};

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
pub(super) const CONTROLS_BAND: f32 = 50.0;
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

/// Camera overlay in a corner of the presented picture.
pub(super) fn overlay_rect(content: egui::Rect, cam_aspect: f32, corner: CamCorner) -> egui::Rect {
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
    let left = match corner {
        CamCorner::BottomLeft | CamCorner::TopLeft => content.left() + margin,
        CamCorner::BottomRight | CamCorner::TopRight => (content.right() - margin - size.x).max(content.left()),
    };
    let top = match corner {
        CamCorner::TopLeft | CamCorner::TopRight => content.top() + margin,
        CamCorner::BottomLeft | CamCorner::BottomRight => (content.bottom() - margin - size.y).max(content.top()),
    };
    egui::Rect::from_min_size(egui::pos2(left, top), size)
}

/// The corner of `content` closest to the middle of a dragged camera.
pub(super) fn nearest_corner(content: egui::Rect, cam: egui::Rect) -> CamCorner {
    let c = cam.center();
    match (c.x < content.center().x, c.y < content.center().y) {
        (true, true) => CamCorner::TopLeft,
        (false, true) => CamCorner::TopRight,
        (true, false) => CamCorner::BottomLeft,
        (false, false) => CamCorner::BottomRight,
    }
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

    /// Grid and Focus buttons plus a "..." menu (Presenter, video only, who is shown) in the
    /// bottom right corner of the stage.
    pub(super) fn stage_controls(&mut self, ui: &mut egui::Ui, area: egui::Rect, ch_id: u64) {
        let mode = self.config.stage_mode;
        let size = egui::vec2(3.0 * 34.0 + 8.0, 40.0);
        let pill = egui::Rect::from_min_size(
            area.right_bottom() - size - egui::vec2(10.0, 10.0),
            size,
        );
        ui.painter()
            .rect_filled(pill, 10.0, egui::Color32::from_black_alpha(170));
        let mut picked = None;
        let mut more = None;
        let mut toggle_idle = false;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(pill.shrink(4.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for m in [StageMode::Grid, StageMode::Focus] {
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
                // Lit while something from the menu is in effect.
                let state = if mode == StageMode::Presenter || self.config.stage_hide_idle {
                    theme::IconState::Active
                } else {
                    theme::IconState::Normal
                };
                more = Some(theme::icon_button(
                    ui,
                    icons::regular::DOTS_THREE,
                    state,
                    "More: Presenter layout, video only, who is shown",
                ));
            },
        );
        let Some(resp) = more else { return };
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
                ui.label(egui::RichText::new("Layout").strong().color(theme::HEADER));
                theme::menu_separator(ui);
                let presenter = StageMode::Presenter;
                let icon = if mode == presenter { icons::regular::CHECK } else { presenter.icon() };
                if theme::menu_item(ui, icon, presenter.label(), false)
                    .on_hover_text(presenter.hint())
                    .clicked()
                {
                    // A toggle: again switches back to the layout before it.
                    picked = Some(if mode == presenter { self.stage_flat } else { presenter });
                }
                let hiding = self.config.stage_hide_idle;
                let icon = if hiding { icons::regular::CHECK_SQUARE } else { icons::regular::SQUARE };
                if theme::menu_item(ui, icon, "Only camera and screen shares", false)
                    .on_hover_text("Hides people without a camera or screen share (only on your screen)")
                    .clicked()
                {
                    toggle_idle = true;
                }
                if mode == StageMode::Grid {
                    return;
                }
                theme::menu_separator(ui);
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
        if toggle_idle {
            self.config.stage_hide_idle = !self.config.stage_hide_idle;
            self.config.save();
        }
        if let Some(m) = picked {
            if m != mode {
                if mode != StageMode::Presenter {
                    self.stage_flat = mode;
                }
                self.config.stage_mode = m;
                self.config.save();
            }
        }
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
        let all = tiles;
        let visible: Vec<(UserInfo, Option<VideoKind>)> = if self.config.stage_hide_idle {
            all.iter().filter(|(_, kind)| kind.is_some()).cloned().collect()
        } else {
            all.to_vec()
        };
        let tiles = &visible[..];
        let controls = all.len() > 1;
        // On a very short stage the buttons float over the corner instead of squeezing the tiles.
        let area = if controls && area.height() >= 2.0 * CONTROLS_BAND + 60.0 {
            egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, (area.max.y - CONTROLS_BAND).max(area.min.y)))
        } else {
            area
        };
        if tiles.is_empty() {
            let p = ui.painter();
            let galley = p.layout_no_wrap(
                "Nobody has a camera or screen share on".into(),
                egui::FontId::proportional(14.0),
                theme::MUTED,
            );
            p.galley(area.center() - galley.size() / 2.0, galley, theme::MUTED);
            if controls {
                self.stage_controls(ui, full, all.first().map_or(0, |(u, _)| u.channel_id));
            }
            return;
        }
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
            // The hint gets its own line above the tiles.
            let grid_area = if hint.is_some() {
                egui::Rect::from_min_max(egui::pos2(area.min.x, (area.min.y + 30.0).min(area.max.y)), area.max)
            } else {
                area
            };
            let (cols, tile) = stage_grid(tiles.len(), grid_area.size(), GAP);
            placed = grid_rects(tiles.len(), cols, tile, GAP, grid_area)
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
        if controls {
            self.stage_controls(ui, full, all.first().map_or(0, |(u, _)| u.channel_id));
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
                    self.watch(user.id, kind);
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
                    *open_fs = Some((user.id, kind));
                }
                // Watching someone else: "stop watching" in the top right corner.
                if !me && (resp.hovered() || ui.rect_contains_pointer(rect)) {
                    let b = egui::Rect::from_center_size(
                        rect.right_top() + egui::vec2(-58.0, 22.0),
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
            None => member_card(ui, rect, user, speaking, me),
        };
        // Hovering a small tile: a button to focus it (the tile itself keeps its own click).
        if !main && ui.rect_contains_pointer(rect) {
            let b = egui::Rect::from_center_size(
                rect.right_top() + egui::vec2(-22.0, 22.0),
                egui::vec2(30.0, 30.0),
            );
            let focus = ui
                .put(
                    b,
                    egui::Button::new(
                        egui::RichText::new(StageMode::Focus.icon())
                            .size(16.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_black_alpha(160))
                    .corner_radius(15.0),
                )
                .on_hover_text("Focus on this");
            if focus.clicked() {
                *focus_click = Some((user.id, kind));
            }
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
        let base = overlay_rect(content, aspect, self.config.presenter_cam_corner);
        let moved = |offset: egui::Vec2| {
            let r = base.translate(offset);
            let dx = (content.left() - r.left()).max(0.0) - (r.right() - content.right()).max(0.0);
            let dy = (content.top() - r.top()).max(0.0) - (r.bottom() - content.bottom()).max(0.0);
            r.translate(egui::vec2(dx, dy))
        };
        let resp = ui.interact(
            moved(self.cam_drag),
            ui.id().with(("presenter_cam", user.id)),
            egui::Sense::click_and_drag(),
        );
        if resp.dragged() {
            self.cam_drag += resp.drag_delta();
        }
        let rect = moved(self.cam_drag);
        if resp.drag_stopped() {
            // Snap to the closest corner.
            self.config.presenter_cam_corner = nearest_corner(content, rect);
            self.config.save();
            self.cam_drag = egui::Vec2::ZERO;
        } else if !resp.dragged() {
            self.cam_drag = egui::Vec2::ZERO;
        }
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
        // Hovering someone else's camera: a small "stop watching" button in the corner.
        if watched && user.id != self.srv.my_id && ui.rect_contains_pointer(rect) && !resp.dragged() {
            let b = egui::Rect::from_center_size(
                rect.right_top() + egui::vec2(-16.0, 16.0),
                egui::vec2(24.0, 24.0),
            );
            let stop = ui
                .put(
                    b,
                    egui::Button::new(
                        egui::RichText::new(icons::regular::EYE_SLASH)
                            .size(13.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_black_alpha(170))
                    .corner_radius(12.0),
                )
                .on_hover_text("Stop watching the camera");
            if stop.clicked() {
                self.unwatch(user.id, VideoKind::Camera);
                return;
            }
        }
        let cursor = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
        let resp = resp.on_hover_cursor(cursor);
        if watched {
            if resp.on_hover_text("Click for fullscreen, drag to move").clicked() {
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

    /// Runs the stage in a 1200x700 screen; returns the stage area and what was drawn.
    fn run_stage_out(
        ctx: &egui::Context,
        app: &mut RustTalkApp,
        tiles: &[(UserInfo, Option<VideoKind>)],
        time: f64,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut area = egui::Rect::NOTHING;
        let out = ctx.run(
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
        (area, out)
    }

    fn run_stage(
        ctx: &egui::Context,
        app: &mut RustTalkApp,
        tiles: &[(UserInfo, Option<VideoKind>)],
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        run_stage_out(ctx, app, tiles, time, events).0
    }

    /// Centre of a drawn text, e.g. a menu entry.
    fn text_pos(out: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        out.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == text => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            _ => None,
        })
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

    /// The pill's buttons: Grid, Focus, "...".
    fn button_pos(area: egui::Rect, n: f32) -> egui::Pos2 {
        let size = egui::vec2(3.0 * 34.0 + 8.0, 40.0);
        let pill = egui::Rect::from_min_size(area.right_bottom() - size - egui::vec2(10.0, 10.0), size);
        egui::pos2(pill.left() + 4.0 + 16.0 + 34.0 * n, pill.center().y)
    }

    #[test]
    fn stage_buttons_switch_layout_and_a_tile_click_focuses_it() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let tiles: Vec<_> = (1..=3).map(|i| (user(i, &format!("user{i}")), None)).collect();
        app.config.stage_mode = StageMode::Grid;
        let mut t = 0.0;
        let area = run_stage(&ctx, &mut app, &tiles, t, vec![]);
        click_at(&ctx, &mut app, &tiles, button_pos(area, 1.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Focus);
        click_at(&ctx, &mut app, &tiles, button_pos(area, 0.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Grid);
        // Presenter lives in the "..." menu.
        click_at(&ctx, &mut app, &tiles, button_pos(area, 2.0), &mut t);
        t += 0.1;
        let (_, out) = run_stage_out(&ctx, &mut app, &tiles, t, vec![]);
        let presenter = text_pos(&out, "Presenter").expect("menu lists Presenter");
        click_at(&ctx, &mut app, &tiles, presenter, &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Presenter);
        // Presenter is a toggle: the same entry goes back to the layout before it (Focus here).
        app.config.stage_mode = StageMode::Presenter;
        app.stage_flat = StageMode::Focus;
        t += 0.1;
        run_stage_out(&ctx, &mut app, &tiles, t, vec![]); // the menu grows: let it settle
        t += 0.1;
        let (_, out) = run_stage_out(&ctx, &mut app, &tiles, t, vec![]);
        let again = text_pos(&out, "Presenter").expect("menu still open");
        click_at(&ctx, &mut app, &tiles, again, &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Focus);
        click_at(&ctx, &mut app, &tiles, button_pos(area, 0.0), &mut t); // closes the open menu
        click_at(&ctx, &mut app, &tiles, button_pos(area, 0.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Grid);
        // Back in Grid, clicking a person tile focuses it.
        let tile_area = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.max.y - CONTROLS_BAND));
        let (cols, tile) = stage_grid(3, tile_area.size(), GAP);
        let first = grid_rects(3, cols, tile, GAP, tile_area)[0];
        assert!(!first.intersects(egui::Rect::from_min_size(
            area.right_bottom() - egui::vec2(160.0, 50.0),
            egui::vec2(160.0, 50.0)
        )), "tiles stay clear of the buttons");
        // The tile itself does not focus ...
        click_at(&ctx, &mut app, &tiles, first.center(), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Grid);
        // ... the button that appears on hover does.
        click_at(&ctx, &mut app, &tiles, first.right_top() + egui::vec2(-22.0, 22.0), &mut t);
        assert_eq!(app.config.stage_mode, StageMode::Focus);
    }

    #[test]
    fn hovering_a_watched_presenter_camera_offers_stop_watching() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let other = user(app.srv.my_id + 7, "other");
        app.watching.insert((other.id, VideoKind::Camera));
        let content = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(1000.0, 562.0));
        let mut t = 0.0;
        let mut frame = |app: &mut RustTalkApp, events: Vec<egui::Event>| {
            t += 0.1;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 700.0))),
                    time: Some(t),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.presenter_overlay(ui, content, &other, false, &mut None);
                    });
                },
            );
        };
        let cam = overlay_rect(content, ASPECT, CamCorner::BottomLeft);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::PointerMoved(cam.center())]);
        let stop = cam.right_top() + egui::vec2(-16.0, 16.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: stop,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![egui::Event::PointerMoved(stop)]);
        frame(&mut app, vec![button(true)]);
        frame(&mut app, vec![button(false)]);
        assert!(!app.watching.contains(&(other.id, VideoKind::Camera)), "stopped watching");
    }

    #[test]
    fn dragging_the_presenter_camera_snaps_it_to_the_closest_corner() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let me = user(app.srv.my_id, "me");
        let content = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(1000.0, 562.0));
        let mut t = 0.0;
        let mut frame = |app: &mut RustTalkApp, events: Vec<egui::Event>| {
            t += 0.1;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 700.0))),
                    time: Some(t),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.presenter_overlay(ui, content, &me, false, &mut None);
                    });
                },
            );
        };
        assert_eq!(app.config.presenter_cam_corner, CamCorner::BottomLeft);
        frame(&mut app, vec![]);
        let start = overlay_rect(content, ASPECT, CamCorner::BottomLeft).center();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![egui::Event::PointerMoved(start)]);
        frame(&mut app, vec![button(start, true)]);
        let target = egui::pos2(content.right() - 150.0, content.top() + 120.0);
        for step in 1..=5 {
            let pos = start + (target - start) * (step as f32 / 5.0);
            frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
        }
        assert_eq!(app.config.presenter_cam_corner, CamCorner::BottomLeft, "only snaps on release");
        frame(&mut app, vec![button(target, false)]);
        assert_eq!(app.config.presenter_cam_corner, CamCorner::TopRight);
        assert_eq!(app.cam_drag, egui::Vec2::ZERO);
        // A plain click (no movement) leaves the corner alone.
        let now = overlay_rect(content, ASPECT, CamCorner::TopRight).center();
        frame(&mut app, vec![egui::Event::PointerMoved(now)]);
        frame(&mut app, vec![button(now, true)]);
        frame(&mut app, vec![button(now, false)]);
        assert_eq!(app.config.presenter_cam_corner, CamCorner::TopRight);
    }

    #[test]
    fn hiding_people_without_video_keeps_the_menu_to_bring_them_back() {
        let ctx = egui::Context::default();
        let mut app = RustTalkApp::with_config(ctx.clone(), ClientConfig::default(), false);
        let tiles: Vec<_> = (1..=3).map(|i| (user(i, &format!("user{i}")), None)).collect();
        app.config.stage_mode = StageMode::Grid;
        let mut t = 0.0;
        let area = run_stage(&ctx, &mut app, &tiles, t, vec![]);
        let toggle = |app: &mut RustTalkApp, t: &mut f64, tiles: &[(UserInfo, Option<VideoKind>)]| {
            click_at(&ctx, app, tiles, button_pos(area, 2.0), t);
            *t += 0.1;
            let (_, out) = run_stage_out(&ctx, app, tiles, *t, vec![]);
            let entry = text_pos(&out, "Only camera and screen shares").expect("menu entry");
            click_at(&ctx, app, tiles, entry, t);
        };
        assert!(!app.config.stage_hide_idle);
        toggle(&mut app, &mut t, &tiles);
        assert!(app.config.stage_hide_idle, "nobody has video: stage is empty but the menu stays");
        // Close the menu, then bring everyone back with the same entry.
        click_at(&ctx, &mut app, &tiles, button_pos(area, 0.0), &mut t);
        toggle(&mut app, &mut t, &tiles);
        assert!(!app.config.stage_hide_idle, "the same entry shows everyone again");
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
            (1700.0, 150.0),
            (1700.0, 90.0),
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
        let r = overlay_rect(content, ASPECT, CamCorner::BottomLeft);
        assert!(inside(content, r));
        assert!((r.width() - 400.0).abs() < 0.5);
        assert!(r.left() - content.left() < 20.0 && content.bottom() - r.bottom() < 20.0);
        // Portrait camera on a flat strip: limited by height, still inside.
        let flat = area(1600.0, 300.0);
        let r = overlay_rect(flat, 9.0 / 16.0, CamCorner::BottomLeft);
        assert!(inside(flat, r) && r.height() <= 300.0 * 0.45 + 0.5);
        // Broken sizes fall back to 16:9.
        assert!(overlay_rect(content, f32::NAN, CamCorner::BottomLeft).width() > 0.0);
        for corner in [CamCorner::BottomLeft, CamCorner::BottomRight, CamCorner::TopLeft, CamCorner::TopRight] {
            let r = overlay_rect(content, ASPECT, corner);
            assert!(inside(content, r), "{corner:?}");
            assert_eq!(nearest_corner(content, r), corner, "a camera in a corner snaps to that corner");
        }
        let near_top_right = egui::Rect::from_center_size(
            egui::pos2(content.right() - 100.0, content.top() + 100.0),
            egui::vec2(400.0, 225.0),
        );
        assert_eq!(nearest_corner(content, near_top_right), CamCorner::TopRight);
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
