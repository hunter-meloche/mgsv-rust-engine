//! `mgsv play`: drive Snake around a test floor with the keyboard, using the
//! player controller, the rig solve and skinning, drawn by the software
//! rasterizer. Everything is read from the local game install.

use std::time::{Duration, Instant};

use anyhow::Result;
use fox_anim::glam::{Mat4, Vec3};
use fox_anim::{HumanRig, Skeleton, SkinnedModel};
use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Window, WindowOptions};
use snake_core::{ClipLibrary, Controller, Input, Stance};

use crate::raster::Raster;

pub struct Assets {
    pub skel: Skeleton,
    pub rig: HumanRig,
    pub clips: ClipLibrary,
    /// (model, base colour)
    pub parts: Vec<(SkinnedModel, Vec3)>,
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

pub fn run(assets: Assets, opts: Options) -> Result<()> {
    let mut ctl = Controller::new(&assets.clips, snake_core::ClipNames::default())
        .map_err(anyhow::Error::msg)?;
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

    let (mut cam_yaw, mut cam_pitch) = (0.0f32, 0.25f32);
    let mut cam_target = Vec3::new(0.0, 1.3, 0.0);
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

        // Camera: arrow keys or left-drag.
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
            cam_pitch = (cam_pitch - turn * 0.5).max(-0.3);
        }
        let mouse = window.get_mouse_pos(MouseMode::Pass);
        if window.get_mouse_down(MouseButton::Left) {
            if let (Some((x, y)), Some((lx, ly))) = (mouse, last_mouse) {
                cam_yaw -= (x - lx) * 0.008;
                cam_pitch = (cam_pitch + (y - ly) * 0.006).clamp(-0.3, 1.2);
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
        };
        if opts.auto {
            // Scripted by frame: run forward, stop, turn around and run back,
            // stop, crouch.
            let f = total_frames;
            input.move_x = 0.0;
            input.move_y = match f {
                0..120 => 1.0,
                180..300 => -1.0,
                _ => 0.0,
            };
            input.crouch = f == 360;
        }
        // Stick-style: diagonal input is not faster.
        let len = input.move_x.hypot(input.move_y);
        if len > 1.0 {
            input.move_x /= len;
            input.move_y /= len;
        }

        while acc >= TICK {
            ctl.update(&assets.rig, &assets.clips, TICK, &input);
            input.crouch = false;
            input.prone = false;
            acc -= TICK;
        }

        // Follow camera (behind and over the right shoulder).
        let eye_height = match ctl.stance {
            Stance::Stand => 1.45,
            Stance::Crouch => 0.95,
            Stance::Prone => 0.55,
        };
        let want = ctl.position + Vec3::new(0.0, eye_height, 0.0);
        cam_target = cam_target.lerp(want, 0.15);
        let forward = Vec3::new(
            cam_yaw.sin() * cam_pitch.cos(),
            -cam_pitch.sin(),
            cam_yaw.cos() * cam_pitch.cos(),
        );
        let right = Vec3::new(-cam_yaw.cos(), 0.0, cam_yaw.sin());
        let focus = cam_target + right * 0.45;
        let eye = focus - forward * 3.2;
        let view = Mat4::look_at_rh(eye, focus, Vec3::Y);
        let proj = Mat4::perspective_rh(
            55f32.to_radians(),
            WIDTH as f32 / HEIGHT as f32,
            0.05,
            200.0,
        );
        let vp = proj * view;

        // Pose and skin.
        let pose = ctl
            .pose(&assets.rig, &assets.skel)
            .map_err(anyhow::Error::msg)?;
        let (rot, pos) = (ctl.rotation(), ctl.position);

        raster.clear(Vec3::new(0.32, 0.42, 0.55), Vec3::new(0.75, 0.72, 0.62));
        let light = Vec3::new(0.4, 0.8, 0.3).normalize();
        // Ground: checkerboard around the player.
        let (cx, cz) = (ctl.position.x.floor() as i32, ctl.position.z.floor() as i32);
        let tiles = (-14..14).flat_map(|i| (-14..14).map(move |j| (i, j)));
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
        for (model, color) in &assets.parts {
            let skinned = model.skin(&pose);
            for (mesh, verts) in model.meshes.iter().zip(&skinned) {
                let world: Vec<Vec3> = verts.iter().map(|v| rot * *v + pos).collect();
                raster.draw(
                    &vp,
                    light,
                    mesh.indices.chunks_exact(3).map(|t| {
                        (
                            [
                                world[t[0] as usize],
                                world[t[1] as usize],
                                world[t[2] as usize],
                            ],
                            *color,
                        )
                    }),
                );
            }
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
        window.set_title(&format!(
            "Snake  |  {:?} {:?}  |  {}  |  {fps} fps  |  WASD move, Shift dash, Ctrl walk, C crouch, Z prone, mouse-drag/QE camera, Esc quit",
            ctl.stance,
            ctl.gait,
            ctl.current_clip()
        ));
    }
    let secs = started.elapsed().as_secs_f32();
    println!(
        "{total_frames} frames in {secs:.1}s ({:.0} fps); final {:?} {:?} at ({:.2}, {:.2}), clip {}",
        total_frames as f32 / secs.max(1e-3),
        ctl.stance,
        ctl.gait,
        ctl.position.x,
        ctl.position.z,
        ctl.current_clip()
    );
    Ok(())
}
