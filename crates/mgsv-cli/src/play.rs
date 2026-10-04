//! `mgsv play`: drive Snake around a test floor with keyboard and mouse,
//! using the player controller, the rig solve, skinning and the weapon
//! systems, drawn by the software rasterizer. Everything is read from the
//! local game install.

use std::time::{Duration, Instant};

use anyhow::Result;
use fox_anim::glam::{Mat4, Quat, Vec3};
use fox_anim::{HumanRig, Pose, Skeleton, SkinnedModel};
use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Window, WindowOptions};
use snake_core::{
    Ballistics, ClipLibrary, Collision, Controller, FlatGround, Input, Stance, WeaponRig,
    WeaponStats,
};

use crate::raster::Raster;

pub struct Gun {
    pub model: SkinnedModel,
    pub rest: Pose,
    pub stats: WeaponStats,
    pub rig: WeaponRig,
    /// Where the rifle rides when not in the hands: bone and offset.
    pub back: (usize, (Quat, Vec3)),
}

pub struct Assets {
    pub skel: Skeleton,
    pub rig: HumanRig,
    pub clips: ClipLibrary,
    /// (model, base colour)
    pub parts: Vec<(SkinnedModel, Vec3)>,
    pub gun: Option<Gun>,
}

/// Automation for smoke tests: stop after `frames`, save the last frame,
/// and optionally feed scripted input instead of the keyboard.
#[derive(Default)]
pub struct Options {
    pub frames: Option<usize>,
    pub shot: Option<std::path::PathBuf>,
    pub auto: bool,
}

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const TICK: f32 = 1.0 / 60.0;
const HELP: &str = "WASD move, Shift dash, Ctrl walk, C crouch, Z prone, RMB aim, LMB fire, R reload, drag/QE camera, Esc quit";

pub fn run(assets: Assets, opts: Options) -> Result<()> {
    let mut ctl = Controller::new(&assets.clips, snake_core::ClipNames::default())
        .map_err(anyhow::Error::msg)?;
    if let Some(g) = &assets.gun {
        ctl.equip(&assets.clips, g.stats.clone(), g.rig.clone())
            .map_err(anyhow::Error::msg)?;
    }
    let mut window = Window::new(
        "Snake (mgsv play)",
        WIDTH,
        HEIGHT,
        WindowOptions {
            resize: false,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);
    let mut raster = Raster::new(WIDTH, HEIGHT);
    let world = FlatGround { height: 0.0 };
    let mut ballistics = Ballistics::default();
    let mut flash = 0u32;

    let (mut cam_yaw, mut cam_pitch) = (0.0f32, 0.25f32);
    let mut cam_target = Vec3::new(0.0, 1.3, 0.0);
    let mut zoom = 0.0f32;
    let mut last_mouse: Option<(f32, f32)> = None;
    let mut last = Instant::now();
    let mut acc = 0.0f32;
    let mut fps_timer = Instant::now();
    let mut frames = 0;
    let mut fps = 0;

    let started = Instant::now();
    let mut total_frames = 0usize;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        if opts.frames.is_some_and(|n| total_frames >= n) {
            break;
        }
        total_frames += 1;
        let now = Instant::now();
        acc += (now - last).as_secs_f32().min(0.25);
        last = now;

        // Camera: arrow keys / Q E, or drag with either mouse button (the
        // right button also aims).
        let turn = 2.5 * TICK;
        if window.is_key_down(Key::Left) || window.is_key_down(Key::Q) {
            cam_yaw += turn;
        }
        if window.is_key_down(Key::Right) || window.is_key_down(Key::E) {
            cam_yaw -= turn;
        }
        if window.is_key_down(Key::Up) {
            cam_pitch = (cam_pitch + turn * 0.5).min(1.2);
        }
        if window.is_key_down(Key::Down) {
            cam_pitch = (cam_pitch - turn * 0.5).max(-0.9);
        }
        let mouse = window.get_mouse_pos(MouseMode::Pass);
        let (lmb, rmb) = (
            window.get_mouse_down(MouseButton::Left),
            window.get_mouse_down(MouseButton::Right),
        );
        let aim_button = rmb;
        if (lmb && !aim_button) || rmb {
            if let (Some((x, y)), Some((lx, ly))) = (mouse, last_mouse) {
                let sens = if aim_button { 0.004 } else { 0.008 };
                cam_yaw -= (x - lx) * sens;
                cam_pitch = (cam_pitch + (y - ly) * sens * 0.75).clamp(-0.9, 1.2);
            }
        }
        last_mouse = mouse;

        let key = |k: Key| window.is_key_down(k);
        let mut input = Input {
            move_x: (key(Key::D) as i32 - key(Key::A) as i32) as f32,
            move_y: (key(Key::W) as i32 - key(Key::S) as i32) as f32,
            camera_yaw: cam_yaw,
            sprint: key(Key::LeftShift) || key(Key::RightShift),
            walk: key(Key::LeftCtrl) || key(Key::RightCtrl),
            crouch: window.is_key_pressed(Key::C, KeyRepeat::No),
            prone: window.is_key_pressed(Key::Z, KeyRepeat::No),
            aim: aim_button,
            fire: aim_button && lmb,
            reload: window.is_key_pressed(Key::R, KeyRepeat::No),
            aim_pitch: -(cam_pitch - ctl.recoil),
        };
        if opts.auto {
            // Scripted by frame: aim and strafe left while firing a burst,
            // then reload.
            let f = total_frames;
            input.move_x = if (60..200).contains(&f) { -1.0 } else { 0.0 };
            input.move_y = 0.0;
            input.aim = f >= 20;
            input.fire = (90..150).contains(&f);
            input.reload = f == 230;
            cam_pitch = 0.05;
            input.aim_pitch = -(cam_pitch - ctl.recoil);
        }
        // Stick-style: diagonal input is not faster.
        let len = input.move_x.hypot(input.move_y);
        if len > 1.0 {
            input.move_x /= len;
            input.move_y /= len;
        }

        while acc >= TICK {
            ctl.update(&assets.rig, &assets.clips, TICK, &input);
            ballistics.step(TICK, &world);
            input.crouch = false;
            input.prone = false;
            input.reload = false;
            acc -= TICK;
        }

        // Follow camera behind the right shoulder; tighter while aiming.
        let target_zoom = if ctl.aiming() { 1.0 } else { 0.0 };
        zoom += (target_zoom - zoom) * 0.2;
        let eye_height = match ctl.stance {
            Stance::Stand => 1.45,
            Stance::Crouch => 0.95,
            Stance::Prone => 0.55,
        };
        let want = ctl.position + Vec3::new(0.0, eye_height + 0.1 * zoom, 0.0);
        cam_target = cam_target.lerp(want, 0.15 + 0.35 * zoom);
        let view_pitch = cam_pitch - ctl.recoil;
        let forward = Vec3::new(
            cam_yaw.sin() * view_pitch.cos(),
            -view_pitch.sin(),
            cam_yaw.cos() * view_pitch.cos(),
        );
        let right = Vec3::new(-cam_yaw.cos(), 0.0, cam_yaw.sin());
        let focus = cam_target + right * (0.45 + 0.2 * zoom);
        let eye = focus - forward * (3.2 - 1.3 * zoom);
        let view = Mat4::look_at_rh(eye, focus, Vec3::Y);
        let fov = 55.0 - 15.0 * zoom;
        let proj =
            Mat4::perspective_rh(fov.to_radians(), WIDTH as f32 / HEIGHT as f32, 0.05, 300.0);
        let vp = proj * view;

        // Pose and skin.
        let pose = ctl
            .pose(&assets.rig, &assets.skel)
            .map_err(anyhow::Error::msg)?;
        let (rot, pos) = (ctl.rotation(), ctl.position);

        // Shots leave the muzzle toward what the crosshair covers.
        let aim_point = world
            .raycast(eye, eye + forward * 300.0)
            .map_or(eye + forward * 300.0, |h| h.point);
        for shot in ctl.take_shots(&pose) {
            let dir = (aim_point - shot.origin).normalize_or(shot.direction);
            ballistics.fire(shot.origin, dir, shot.speed, shot.gravity);
            flash = 2;
        }

        raster.clear(Vec3::new(0.32, 0.42, 0.55), Vec3::new(0.75, 0.72, 0.62));
        let light = Some(Vec3::new(0.4, 0.8, 0.3).normalize());
        // Ground: checkerboard around the player.
        let (cx, cz) = (ctl.position.x.floor() as i32, ctl.position.z.floor() as i32);
        let tiles = (-20..20).flat_map(|i| (-20..20).map(move |j| (i, j)));
        raster.draw(
            &vp,
            light,
            tiles.flat_map(|(i, j)| {
                let (x, z) = ((cx + i) as f32, (cz + j) as f32);
                let c = if (cx + i + cz + j).rem_euclid(2) == 0 {
                    Vec3::new(0.42, 0.40, 0.33)
                } else {
                    Vec3::new(0.50, 0.47, 0.38)
                };
                let p = [
                    Vec3::new(x, 0.0, z),
                    Vec3::new(x + 1.0, 0.0, z),
                    Vec3::new(x + 1.0, 0.0, z + 1.0),
                    Vec3::new(x, 0.0, z + 1.0),
                ];
                [([p[0], p[1], p[2]], c), ([p[0], p[2], p[3]], c)]
            }),
        );
        // Bullet holes.
        raster.draw(
            &vp,
            light,
            ballistics.impacts.iter().flat_map(|(hit, _)| {
                let p = hit.point + Vec3::Y * 0.003;
                let (a, b) = (Vec3::X * 0.05, Vec3::Z * 0.05);
                let c = Vec3::splat(0.12);
                [
                    ([p - a - b, p + a - b, p + a + b], c),
                    ([p - a - b, p + a + b, p - a + b], c),
                ]
            }),
        );
        let to_world = |v: Vec3| rot * v + pos;
        for (model, color) in &assets.parts {
            let skinned = model.skin(&pose);
            for (mesh, verts) in model.meshes.iter().zip(&skinned) {
                let world_v: Vec<Vec3> = verts.iter().map(|v| to_world(*v)).collect();
                raster.draw(
                    &vp,
                    light,
                    mesh.indices.chunks_exact(3).map(|t| {
                        (
                            [
                                world_v[t[0] as usize],
                                world_v[t[1] as usize],
                                world_v[t[2] as usize],
                            ],
                            *color,
                        )
                    }),
                );
            }
        }
        // The rifle: in the hands while aiming/reloading, else on the back.
        if let (Some(g), Some(w)) = (&assets.gun, &ctl.weapon) {
            let (gr, gp) = if ctl.weapon_in_hand() {
                w.rig.gun_transform(&pose)
            } else {
                let (bone, (r, t)) = g.back;
                (
                    pose.rotations[bone] * r,
                    pose.positions[bone] + pose.rotations[bone] * t,
                )
            };
            let skinned = g.model.skin(&g.rest);
            for (mesh, verts) in g.model.meshes.iter().zip(&skinned) {
                let world_v: Vec<Vec3> = verts.iter().map(|v| to_world(gp + gr * *v)).collect();
                raster.draw(
                    &vp,
                    light,
                    mesh.indices.chunks_exact(3).map(|t| {
                        (
                            [
                                world_v[t[0] as usize],
                                world_v[t[1] as usize],
                                world_v[t[2] as usize],
                            ],
                            Vec3::splat(0.22),
                        )
                    }),
                );
            }
            if flash > 0 && ctl.weapon_in_hand() {
                flash -= 1;
                let m = to_world(gp + gr * w.rig.muzzle);
                let f = rot * gr * Vec3::Z;
                let side = f.cross(Vec3::Y).normalize_or(Vec3::X) * 0.07;
                let up = side.cross(f).normalize_or(Vec3::Y) * 0.07;
                let tip = m + f * 0.22;
                let c = Vec3::new(1.0, 0.85, 0.4);
                raster.draw(
                    &vp,
                    None,
                    [
                        ([m - side, m + side, tip], c),
                        ([m - up, m + up, tip], c),
                        ([m - side, m + up, m + side], c),
                    ]
                    .into_iter(),
                );
            }
        }
        // Tracers: thin quads facing the camera.
        raster.draw(
            &vp,
            None,
            ballistics.bullets.iter().flat_map(|b| {
                let (a, z) = (b.previous, b.position);
                let side = (z - a).cross(eye - a).normalize_or(Vec3::X) * 0.012;
                let c = Vec3::new(1.0, 0.9, 0.55);
                [
                    ([a - side, a + side, z + side], c),
                    ([a - side, z + side, z - side], c),
                ]
            }),
        );
        if ctl.aiming() {
            raster.crosshair(0xFFFFFF);
        }
        window.update_with_buffer(&raster.color, WIDTH, HEIGHT)?;
        if opts.frames.is_some_and(|n| total_frames >= n) {
            if let Some(path) = &opts.shot {
                crate::raster::save_png(&raster, path)?;
            }
        }

        frames += 1;
        if fps_timer.elapsed() >= Duration::from_secs(1) {
            fps = frames;
            frames = 0;
            fps_timer = Instant::now();
        }
        let ammo = ctl
            .weapon
            .as_ref()
            .map(|w| {
                format!(
                    "{} {}/{}{}",
                    w.stats.weapon,
                    w.ammo,
                    w.reserve,
                    if w.reloading() { " (reloading)" } else { "" }
                )
            })
            .unwrap_or_default();
        window.set_title(&format!(
            "Snake  |  {:?} {:?}  |  {ammo}  |  {}  |  {fps} fps  |  {HELP}",
            ctl.stance,
            ctl.gait,
            ctl.current_clip()
        ));
    }
    let secs = started.elapsed().as_secs_f32();
    let ammo = ctl
        .weapon
        .as_ref()
        .map(|w| format!(", ammo {}/{}", w.ammo, w.reserve))
        .unwrap_or_default();
    println!(
        "{total_frames} frames in {secs:.1}s ({:.0} fps); final {:?} {:?} at ({:.2}, {:.2}), clip {}{ammo}, {} impacts",
        total_frames as f32 / secs.max(1e-3),
        ctl.stance,
        ctl.gait,
        ctl.position.x,
        ctl.position.z,
        ctl.current_clip(),
        ballistics.impacts.len()
    );
    Ok(())
}
