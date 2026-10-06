//! C API for Snake's player systems, for hosts such as an SKSE (Skyrim)
//! plugin. See `include/snake.h` for the C declarations and
//! `docs/BRIDGE.md` for how a host is expected to use it.
//!
//! Conventions: Fox space (Y up, meters, a heading of 0 faces +Z, the right
//! side of a heading is -X); quaternions are `x, y, z, w`. Every function is
//! panic-safe: failures return NULL / 0 and set `snake_last_error()`.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use fox_anim::{Pose, Quat, Vec3};
use snake_core::{Collision, Controller, Gait, Hit, Input, Stance};
use snake_runtime::SnakeAssets;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeQuat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl From<Vec3> for SnakeVec3 {
    fn from(v: Vec3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}
impl From<SnakeVec3> for Vec3 {
    fn from(v: SnakeVec3) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }
}

/// One tick of input. Booleans are 0/1; `switch_to` is a weapon slot or -1.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeInput {
    pub move_x: f32,
    pub move_y: f32,
    pub camera_yaw: f32,
    pub aim_pitch: f32,
    pub sprint: u8,
    pub walk: u8,
    pub crouch: u8,
    pub prone: u8,
    pub aim: u8,
    pub fire: u8,
    pub reload: u8,
    pub dive: u8,
    pub switch_to: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeHit {
    pub point: SnakeVec3,
    pub normal: SnakeVec3,
    /// Host id of the object hit, or -1.
    pub id: i64,
}

/// The host's world. `raycast` returns 1 and fills `hit` when the segment
/// `from -> to` hits something. `slide` moves a character capsule (feet
/// position) by `delta`, sliding along obstacles, writing the new feet
/// position to `out`; NULL means no obstacles. Arguments are passed by
/// pointer so any FFI (C, C++, C#, Python ctypes) can implement them.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SnakeWorld {
    pub user: *mut c_void,
    pub raycast: Option<
        unsafe extern "C" fn(*mut c_void, *const SnakeVec3, *const SnakeVec3, *mut SnakeHit) -> i32,
    >,
    pub slide: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *const SnakeVec3,
            *const SnakeVec3,
            f32,
            f32,
            *mut SnakeVec3,
        ),
    >,
}

impl Collision for SnakeWorld {
    fn raycast(&self, from: Vec3, to: Vec3) -> Option<Hit> {
        let f = self.raycast?;
        let mut hit = SnakeHit::default();
        // SAFETY: the host promised a valid callback and user pointer.
        let (from, to): (SnakeVec3, SnakeVec3) = (from.into(), to.into());
        (unsafe { f(self.user, &from, &to, &mut hit) } != 0).then(|| Hit {
            point: hit.point.into(),
            normal: hit.normal.into(),
            id: usize::try_from(hit.id).ok(),
        })
    }

    fn slide(&self, feet: Vec3, delta: Vec3, radius: f32, height: f32) -> Vec3 {
        match self.slide {
            // SAFETY: as above.
            Some(f) => {
                let (feet_c, delta_c): (SnakeVec3, SnakeVec3) = (feet.into(), delta.into());
                let mut out = SnakeVec3::from(feet + delta);
                unsafe { f(self.user, &feet_c, &delta_c, radius, height, &mut out) };
                out.into()
            }
            None => feet + delta,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeShot {
    pub origin: SnakeVec3,
    pub direction: SnakeVec3,
    pub speed: f32,
    pub gravity: f32,
    /// MGSV damage per hit: lethal (MRS-4: 600) and sleep (WU S.Pistol: 750).
    pub lethal_damage: f32,
    pub sleep_damage: f32,
}

/// Stance: 0 stand, 1 crouch, 2 prone. Gait: 0 idle, 1 walk, 2 run, 3 dash.
/// `weapon` is the slot in use or -1.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SnakeStatus {
    pub stance: i32,
    pub gait: i32,
    pub aiming: i32,
    pub weapon: i32,
    pub ammo: i32,
    pub reserve: i32,
    pub reloading: i32,
    pub recoil: f32,
    /// Magazine capacity of the weapon in use.
    pub magazine: i32,
    /// Reload progress 0..1 (0 when not reloading).
    pub reload_progress: f32,
    /// Reloads do not use up the reserve.
    pub unlimited: i32,
    /// Height of Snake's head above his feet (metres; standing ~1.6, prone ~0.4).
    pub head_height: f32,
}

/// Loaded assets, shared by every player created from it.
pub struct SnakeRuntime {
    assets: Arc<SnakeAssets>,
    bone_names: Vec<Option<CString>>,
    /// Skyrim mesh path of each weapon (relative to `Data\meshes`).
    weapon_meshes: Vec<CString>,
    /// Skyrim mesh path of each weapon's magazine (empty if none).
    magazine_meshes: Vec<CString>,
    weapon_names: Vec<CString>,
    /// Per weapon, the body shapes that show it in its closed holster.
    holster_shapes: Vec<Vec<CString>>,
    /// MGSV's common sound bank, for Snake's foley (None if it failed to
    /// load).
    foley: std::sync::Mutex<Option<snake_runtime::foley::FoleyBank>>,
}

pub struct SnakePlayer {
    assets: Arc<SnakeAssets>,
    controller: Controller,
    pose: Pose,
    /// The strap at Snake's back, simulated on top of the pose.
    strap: Option<snake_core::strap::StrapSim>,
    /// The rifle's sling at Snake's left hip, simulated like the strap.
    sling: Option<snake_core::sling::SlingSim>,
    /// Foley requests since the host last took them, and the tracker that
    /// finds footsteps in clips without motion events.
    foley_tracker: Option<snake_core::foley::FoleyTracker>,
    foley: Vec<snake_core::foley::Foley>,
    /// Rendered foley handed to the host (valid until the next call).
    foley_pcm: Vec<(Vec<i16>, i32)>,
    /// Shots fired during updates, until the host takes them.
    shots: Vec<snake_core::Shot>,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: impl std::fmt::Display) {
    let text = CString::new(msg.to_string().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = text);
}

/// Run `f`, turning errors and panics into `fallback` plus a stored message.
fn guard<T>(fallback: T, f: impl FnOnce() -> Result<T, String>) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            set_error(e);
            fallback
        }
        Err(_) => {
            set_error("internal panic");
            fallback
        }
    }
}

unsafe fn path_arg(p: *const c_char) -> Result<String, String> {
    if p.is_null() {
        return Err("null path".into());
    }
    // SAFETY: caller passes a NUL-terminated string.
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map(str::to_string)
        .map_err(|e| e.to_string())
}

/// The last error message on this thread (empty if none). Valid until the
/// next failing call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn snake_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Load everything from a game install. `dictionaries_dir` is the repo's
/// `dictionaries/` folder (needed to find clips by name). NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_runtime_open(
    game_dir: *const c_char,
    dictionaries_dir: *const c_char,
) -> *mut SnakeRuntime {
    guard(std::ptr::null_mut(), || {
        // SAFETY: forwarded caller contract.
        let (game, dicts) = unsafe { (path_arg(game_dir)?, path_arg(dictionaries_dir)?) };
        let vfs = snake_runtime::open_game(Path::new(&game), Path::new(&dicts))
            .map_err(|e| format!("{e:#}"))?;
        let assets = SnakeAssets::load(&vfs).map_err(|e| format!("{e:#}"))?;
        let mut names = fox_data::NameTable::new();
        let _ = names.load_dictionary(Path::new(&dicts).join("mtar_tools/skl_dictionary.txt"));
        let bone_names = assets
            .skel
            .bones
            .iter()
            .map(|b| names.resolve32(b.name).and_then(|n| CString::new(n).ok()))
            .collect();
        let weapon_meshes = assets
            .weapons
            .iter()
            .map(|w| CString::new(snake_runtime::skyrim_export::mesh_path(w.id)).unwrap_or_default())
            .collect();
        let magazine_meshes = assets
            .weapons
            .iter()
            .map(|w| {
                let path = if w.rig.magazine.is_some() {
                    snake_runtime::skyrim_export::magazine_mesh_path(w.id)
                } else {
                    String::new()
                };
                CString::new(path).unwrap_or_default()
            })
            .collect();
        let weapon_names = assets
            .weapons
            .iter()
            .map(|w| CString::new(w.name).unwrap_or_default())
            .collect();
        let holster_shapes = assets
            .weapons
            .iter()
            .map(|w| {
                w.holster_meshes
                    .iter()
                    .map(|&m| CString::new(snake_runtime::skyrim_body::shape_name("body", m)).unwrap_or_default())
                    .collect()
            })
            .collect();
        // Foley is optional: without the bank Snake is just quiet.
        let foley = snake_runtime::foley::FoleyBank::load(&vfs).ok();
        Ok(Box::into_raw(Box::new(SnakeRuntime {
            assets: Arc::new(assets),
            bone_names,
            weapon_meshes,
            magazine_meshes,
            weapon_names,
            holster_shapes,
            foley: std::sync::Mutex::new(foley),
        })))
    })
}

/// Free a runtime. Players created from it stay valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_runtime_free(rt: *mut SnakeRuntime) {
    if !rt.is_null() {
        // SAFETY: pointer came from snake_runtime_open.
        drop(unsafe { Box::from_raw(rt) });
    }
}

/// Number of skeleton bones (the length of pose arrays).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_bone_count(rt: *const SnakeRuntime) -> u32 {
    // SAFETY: valid runtime pointer or null.
    unsafe { rt.as_ref() }.map_or(0, |r| r.assets.skel.bones.len() as u32)
}

/// Bone name (e.g. `SKL_023_RHAND`), or NULL if unknown. Owned by the runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_bone_name(rt: *const SnakeRuntime, bone: u32) -> *const c_char {
    // SAFETY: valid runtime pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.bone_names.get(bone as usize)?.as_ref())
        .map_or(std::ptr::null(), |n| n.as_ptr())
}

/// Parent bone index, or -1 for the root (and for bad arguments).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_bone_parent(rt: *const SnakeRuntime, bone: u32) -> i32 {
    // SAFETY: valid runtime pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.assets.skel.bones.get(bone as usize)?.parent)
        .map_or(-1, |p| p as i32)
}

/// Number of carried weapons (slots for `switch_to`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_count(rt: *const SnakeRuntime) -> u32 {
    // SAFETY: valid runtime pointer or null.
    unsafe { rt.as_ref() }.map_or(0, |r| r.assets.weapons.len() as u32)
}

/// Skyrim mesh of weapon `i` (relative to `Data\meshes`, written by
/// `mgsv skyrim-export`), or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_mesh(rt: *const SnakeRuntime, i: u32) -> *const c_char {
    // SAFETY: valid runtime pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.weapon_meshes.get(i as usize))
        .map_or(std::ptr::null(), |s| s.as_ptr())
}

/// Display name of weapon `i` (e.g. "AM MRS-4"), or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_name(rt: *const SnakeRuntime, i: u32) -> *const c_char {
    // SAFETY: valid pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.weapon_names.get(i as usize))
        .map_or(std::ptr::null(), |s| s.as_ptr())
}

/// Name of the `k`-th shape of Snake's body mesh that shows weapon `i` in
/// its closed holster (null past the last): the host shows these only
/// while the weapon is in there (snake_retarget_weapon_place returns 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_holster_shape(rt: *const SnakeRuntime, i: u32, k: u32) -> *const c_char {
    // SAFETY: valid pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.holster_shapes.get(i as usize))
        .and_then(|v| v.get(k as usize))
        .map_or(std::ptr::null(), |s| s.as_ptr())
}

/// Skyrim mesh path of weapon `i`'s magazine, or null if it has none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_magazine_mesh(rt: *const SnakeRuntime, i: u32) -> *const c_char {
    // SAFETY: valid pointer or null.
    unsafe { rt.as_ref() }
        .and_then(|r| r.magazine_meshes.get(i as usize))
        .filter(|s| !s.as_bytes().is_empty())
        .map_or(std::ptr::null(), |s| s.as_ptr())
}

/// Muzzle of weapon `i` in its exported Skyrim mesh's frame (Skyrim axes and
/// units); the barrel points along +Y. Returns 0 if there is no such weapon.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_weapon_muzzle(rt: *const SnakeRuntime, i: u32, out: *mut SnakeVec3) -> i32 {
    // SAFETY: valid runtime pointer or null; `out` valid or null.
    let Some(w) = unsafe { rt.as_ref() }.and_then(|r| r.assets.weapons.get(i as usize)) else {
        return 0;
    };
    let m = snake_runtime::skyrim_export::muzzle_in_mesh(w.rig.muzzle.to_array());
    if let Some(out) = unsafe { out.as_mut() } {
        *out = SnakeVec3 { x: m[0], y: m[1], z: m[2] };
    }
    1
}

/// A new player at the origin with every weapon equipped. NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_new(rt: *const SnakeRuntime) -> *mut SnakePlayer {
    guard(std::ptr::null_mut(), || {
        // SAFETY: valid runtime pointer or null.
        let rt = unsafe { rt.as_ref() }.ok_or("null runtime")?;
        let assets = rt.assets.clone();
        let controller = assets.controller().map_err(|e| format!("{e:#}"))?;
        let pose = controller.pose(&assets.rig, &assets.skel)?;
        let strap = snake_core::strap::StrapSim::new(&assets.skel);
        let sling = snake_core::sling::SlingSim::new(&assets.skel);
        let foley_tracker = snake_core::foley::FoleyTracker::new(&assets.skel);
        Ok(Box::into_raw(Box::new(SnakePlayer {
            assets,
            controller,
            pose,
            strap,
            sling,
            foley_tracker,
            foley: Vec::new(),
            foley_pcm: Vec::new(),
            shots: Vec::new(),
        })))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_free(p: *mut SnakePlayer) {
    if !p.is_null() {
        // SAFETY: pointer came from snake_player_new.
        drop(unsafe { Box::from_raw(p) });
    }
}

/// Advance the player by `dt` seconds (call at a fixed rate, e.g. 60 Hz).
/// `world` may be NULL (flat, obstacle-free, no bullet hits). Returns 1 on
/// success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_update(
    p: *mut SnakePlayer,
    input: *const SnakeInput,
    world: *const SnakeWorld,
    dt: f32,
) -> i32 {
    guard(0, || {
        // SAFETY: valid pointers or null.
        let p = unsafe { p.as_mut() }.ok_or("null player")?;
        let i = unsafe { input.as_ref() }.copied().unwrap_or_default();
        let input = Input {
            move_x: i.move_x,
            move_y: i.move_y,
            camera_yaw: i.camera_yaw,
            sprint: i.sprint != 0,
            walk: i.walk != 0,
            crouch: i.crouch != 0,
            prone: i.prone != 0,
            aim: i.aim != 0,
            fire: i.fire != 0,
            reload: i.reload != 0,
            dive: i.dive != 0,
            aim_pitch: i.aim_pitch,
            switch_to: usize::try_from(i.switch_to).ok(),
        };
        let flat = snake_core::FlatGround {
            height: f32::NEG_INFINITY,
        };
        // SAFETY: valid pointer or null.
        let collision: &dyn Collision = match unsafe { world.as_ref() } {
            Some(w) => w,
            None => &flat,
        };
        p.controller
            .update(&p.assets.rig, &p.assets.clips, collision, dt, &input);
        p.pose = p.controller.pose(&p.assets.rig, &p.assets.skel)?;
        if let Some(sling) = p.sling.as_mut() {
            sling.step(&mut p.pose, p.controller.position, p.controller.rotation(), dt);
        }
        if let Some(strap) = p.strap.as_mut() {
            strap.step(&mut p.pose, p.controller.position, p.controller.rotation(), dt);
        }
        let crossed = p.controller.take_motion_events();
        let handling = p.controller.take_foley();
        p.foley.extend(handling);
        if let Some(t) = p.foley_tracker.as_mut() {
            let moving = p.controller.gait != Gait::Idle;
            let has_feet = p.controller.clip_has_foot_events();
            let action = p.controller.foley_action();
            t.update(&crossed, has_feet, moving, action, &p.pose, dt, &mut p.foley);
            // Nobody is taking them: keep only the latest.
            if p.foley.len() > 64 {
                p.foley.drain(..p.foley.len() - 64);
            }
        }
        p.controller.track_magazine(&p.pose);
        let shots = p.controller.take_shots(&p.pose, collision);
        p.shots.extend(shots);
        Ok(1)
    })
}

/// The character root: feet position and heading (radians about +Y).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_transform(
    p: *const SnakePlayer,
    position: *mut SnakeVec3,
    yaw: *mut f32,
) {
    // SAFETY: valid pointers or null.
    if let Some(p) = unsafe { p.as_ref() } {
        if let Some(out) = unsafe { position.as_mut() } {
            *out = p.controller.position.into();
        }
        if let Some(out) = unsafe { yaw.as_mut() } {
            *out = p.controller.yaw;
        }
    }
}

/// Teleport the character (e.g. to the host's spawn point).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_set_transform(
    p: *mut SnakePlayer,
    position: SnakeVec3,
    yaw: f32,
) {
    // SAFETY: valid pointer or null.
    if let Some(p) = unsafe { p.as_mut() } {
        p.controller.position = position.into();
        p.controller.yaw = yaw;
    }
}

/// Bone transforms from the last update, in the character's root space
/// (apply the root transform to place them in the world). Writes up to
/// `capacity` entries to each array; returns the number of bones.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_pose(
    p: *const SnakePlayer,
    positions: *mut SnakeVec3,
    rotations: *mut SnakeQuat,
    capacity: u32,
) -> u32 {
    // SAFETY: valid pointer or null; arrays hold `capacity` entries.
    let Some(p) = (unsafe { p.as_ref() }) else {
        return 0;
    };
    let n = p.pose.positions.len().min(capacity as usize);
    for i in 0..n {
        if !positions.is_null() {
            unsafe { *positions.add(i) = p.pose.positions[i].into() };
        }
        if !rotations.is_null() {
            let q: Quat = p.pose.rotations[i];
            unsafe {
                *rotations.add(i) = SnakeQuat {
                    x: q.x,
                    y: q.y,
                    z: q.z,
                    w: q.w,
                }
            };
        }
    }
    p.pose.positions.len() as u32
}

/// Shots fired since the last call, in world space. Writes up to `capacity`;
/// returns the number written (extra shots are dropped).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_take_shots(
    p: *mut SnakePlayer,
    out: *mut SnakeShot,
    capacity: u32,
) -> u32 {
    // SAFETY: valid pointer or null; `out` holds `capacity` entries.
    let Some(p) = (unsafe { p.as_mut() }) else {
        return 0;
    };
    let shots = std::mem::take(&mut p.shots);
    let n = shots.len().min(capacity as usize);
    if !out.is_null() {
        for (i, s) in shots.iter().take(n).enumerate() {
            unsafe {
                *out.add(i) = SnakeShot {
                    origin: s.origin.into(),
                    direction: s.direction.into(),
                    speed: s.speed,
                    gravity: s.gravity,
                    lethal_damage: s.lethal_damage,
                    sleep_damage: s.sleep_damage,
                }
            };
        }
    }
    n as u32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_status(p: *const SnakePlayer, out: *mut SnakeStatus) {
    // SAFETY: valid pointers or null.
    let (Some(p), Some(out)) = (unsafe { p.as_ref() }, unsafe { out.as_mut() }) else {
        return;
    };
    let c = &p.controller;
    let w = c.weapon();
    *out = SnakeStatus {
        stance: match c.stance {
            Stance::Stand => 0,
            Stance::Crouch => 1,
            Stance::Prone => 2,
        },
        gait: match c.gait {
            Gait::Idle => 0,
            Gait::Walk => 1,
            Gait::Run => 2,
            Gait::Dash => 3,
        },
        aiming: c.aiming() as i32,
        weapon: c.current.map_or(-1, |i| i as i32),
        ammo: w.map_or(0, |w| w.ammo as i32),
        reserve: w.map_or(0, |w| w.reserve as i32),
        reloading: w.is_some_and(|w| w.reloading()) as i32,
        recoil: c.recoil,
        magazine: w.map_or(0, |w| w.stats.magazine as i32),
        reload_progress: w.and_then(|w| w.reload_progress()).unwrap_or(0.0),
        unlimited: c.unlimited_magazines as i32,
        head_height: p
            .assets
            .skel
            .find(fox_hash::str_code32(b"SKL_004_HEAD"))
            .and_then(|i| p.pose.positions.get(i))
            .map_or(0.0, |v| v.y),
    };
}

/// Name of the clip Snake's controller is playing (diagnostics), copied into
/// `buf` (NUL-terminated, truncated to `len`). Returns the full length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_clip(p: *const SnakePlayer, buf: *mut c_char, len: u32) -> u32 {
    // SAFETY: valid pointers or null; `buf` holds `len` bytes.
    let Some(p) = (unsafe { p.as_ref() }) else { return 0 };
    let name = p.controller.current_clip().as_bytes();
    if !buf.is_null() && len > 0 {
        let n = name.len().min(len as usize - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(name.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
    }
    name.len() as u32
}

/// Unlimited magazines: reloads never use up the reserve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_set_unlimited_magazines(p: *mut SnakePlayer, on: i32) {
    // SAFETY: valid pointer or null.
    if let Some(p) = unsafe { p.as_mut() } {
        p.controller.unlimited_magazines = on != 0;
    }
}

/// One rendered foley sound: 48 kHz stereo 16-bit samples (interleaved).
#[repr(C)]
pub struct SnakeSound {
    pub samples: *const i16,
    /// Sample frames (pairs).
    pub frames: u32,
    /// What made it (snake_core::foley::FoleyKind).
    pub kind: i32,
}

/// Snake's foley since the last call (footsteps, hands, suit and weapon
/// rattle, the chest landing), rendered from MGSV's sound bank for ground
/// `surface` (an MGSV footstep symbol: SOILA, ROCKA, WOODA, IRONA, GRASA,
/// GRAVA, SANDA, ...) and `wet` ground. Writes up to `max` sounds to `out`;
/// their samples stay valid until the next call. Returns the count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_foley(
    rt: *const SnakeRuntime,
    p: *mut SnakePlayer,
    surface: *const c_char,
    wet: i32,
    out: *mut SnakeSound,
    max: u32,
) -> u32 {
    guard(0, || {
        // SAFETY: valid pointers or null; `surface` NUL-terminated or null.
        let (rt, p) = unsafe { (rt.as_ref().ok_or("null runtime")?, p.as_mut().ok_or("null player")?) };
        let surface = if surface.is_null() {
            "SOILA".to_string()
        } else {
            unsafe { CStr::from_ptr(surface) }.to_string_lossy().into_owned()
        };
        p.foley_pcm.clear();
        let pending = std::mem::take(&mut p.foley);
        let mut bank = rt.foley.lock().map_err(|_| "foley bank poisoned")?;
        let Some(bank) = bank.as_mut() else { return Ok(0) };
        let weapon = p
            .controller
            .weapon()
            .and_then(|w| snake_runtime::foley::weapon_symbol(&w.stats.sound_set));
        for f in pending {
            if p.foley_pcm.len() >= max as usize {
                break;
            }
            if let Some(pcm) = bank.render(&f, &surface, wet != 0, weapon) {
                p.foley_pcm.push((pcm.samples, f.kind as i32));
            }
        }
        for (k, (samples, kind)) in p.foley_pcm.iter().enumerate() {
            // SAFETY: the caller provides room for `max` entries.
            unsafe {
                *out.add(k) = SnakeSound {
                    samples: samples.as_ptr(),
                    frames: (samples.len() / 2) as u32,
                    kind: *kind,
                };
            }
        }
        Ok(p.foley_pcm.len() as u32)
    })
}

/// Tempo of all of Snake's motion (1 = MGSV's; 1.5 = half again as fast,
/// reloads included; the fire rate is unchanged).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_player_set_tempo(p: *mut SnakePlayer, tempo: f32) {
    // SAFETY: valid pointer or null.
    if let Some(p) = unsafe { p.as_mut() } {
        p.controller.tempo = tempo.clamp(0.1, 4.0);
    }
}

/// Where weapon `weapon`'s magazine goes this frame, like
/// [`snake_retarget_weapon_place`]: 1 = relative to "NPC R Hand [RHnd]"
/// (in the gun, or in Snake's left hand during a reload), 2 = relative to
/// "NPC Spine2 [Spn2]" (in the holstered gun), 0 = not shown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_magazine_place(
    r: *const SnakeRetarget,
    p: *const SnakePlayer,
    weapon: u32,
    translation: *mut SnakeVec3,
    rotation: *mut SnakeQuat,
) -> i32 {
    guard(0, || {
        // SAFETY: valid pointers or null.
        let (r, p) = unsafe {
            (
                r.as_ref().ok_or("null retarget")?,
                p.as_ref().ok_or("null player")?,
            )
        };
        let w = p.assets.weapons.get(weapon as usize).ok_or("no such weapon")?;
        let Some(m) = w.rig.magazine else { return Ok(0) };
        let ctl = &p.controller;
        let Some((gr, gp, in_hand)) = ctl.gun_place(weapon as usize, &p.pose) else {
            return Ok(0);
        };
        let held = if ctl.current == Some(weapon as usize) {
            let Some((q, t, _)) = ctl.magazine() else { return Ok(0) };
            (q, t)
        } else {
            (gr * m.cnp.0, gp + gr * m.cnp.1)
        };
        let (place, bone) = if in_hand { (1, "NPC R Hand [RHnd]") } else { (2, "NPC Spine2 [Spn2]") };
        let Some((q, t)) = r.retarget.attach_local(&p.pose, bone, held) else {
            return Ok(0);
        };
        unsafe {
            if !translation.is_null() {
                *translation = t.into();
            }
            if !rotation.is_null() {
                *rotation = SnakeQuat { x: q.x, y: q.y, z: q.z, w: q.w };
            }
        }
        Ok(place)
    })
}

/// Retargeting onto Skyrim's character skeleton.
pub struct SnakeRetarget {
    retarget: snake_runtime::skyrim::SkyrimRetarget,
    names: Vec<CString>,
}

/// Build the retarget table from the Skyrim install at `skyrim_dir` (reads
/// its character `skeleton.nif` from the BSAs). NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_new(
    rt: *const SnakeRuntime,
    skyrim_dir: *const c_char,
) -> *mut SnakeRetarget {
    guard(std::ptr::null_mut(), || {
        // SAFETY: valid runtime pointer or null; NUL-terminated path.
        let rt = unsafe { rt.as_ref() }.ok_or("null runtime")?;
        let dir = unsafe { path_arg(skyrim_dir)? };
        let data = skyrim_data::read_install_file(Path::new(&dir), skyrim_data::CHARACTER_SKELETON)
            .map_err(|e| e.to_string())?;
        let nif = skyrim_data::Nif::parse(&data).map_err(|e| e.to_string())?;
        let retarget = snake_runtime::skyrim::SkyrimRetarget::new(&nif, &rt.assets.skel)
            .map_err(|e| format!("{e:#}"))?;
        let names = retarget
            .names()
            .map(|n| CString::new(n).unwrap_or_default())
            .collect();
        Ok(Box::into_raw(Box::new(SnakeRetarget { retarget, names })))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_free(r: *mut SnakeRetarget) {
    if !r.is_null() {
        // SAFETY: pointer came from snake_retarget_new.
        drop(unsafe { Box::from_raw(r) });
    }
}

/// Number of Skyrim bones written by snake_retarget_apply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_count(r: *const SnakeRetarget) -> u32 {
    // SAFETY: valid pointer or null.
    unsafe { r.as_ref() }.map_or(0, |r| r.names.len() as u32)
}

/// Skyrim node name of output bone `i` (e.g. `NPC L UpperArm [LUar]`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_bone(r: *const SnakeRetarget, i: u32) -> *const c_char {
    // SAFETY: valid pointer or null.
    unsafe { r.as_ref() }
        .and_then(|r| r.names.get(i as usize))
        .map_or(std::ptr::null(), |n| n.as_ptr())
}

/// Where weapon `weapon`'s exported mesh goes this frame: returns 1 when it
/// is in Snake's hands (transform relative to Skyrim's `NPC R Hand [RHnd]`),
/// 2 when it is holstered (relative to `NPC Spine2 [Spn2]`), 0 when it is
/// not shown (no holster point). Skyrim axes and units.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_weapon_place(
    r: *const SnakeRetarget,
    p: *const SnakePlayer,
    weapon: u32,
    translation: *mut SnakeVec3,
    rotation: *mut SnakeQuat,
) -> i32 {
    guard(0, || {
        // SAFETY: valid pointers or null.
        let (r, p) = unsafe {
            (
                r.as_ref().ok_or("null retarget")?,
                p.as_ref().ok_or("null player")?,
            )
        };
        let Some((gr, gp, in_hand)) = p.controller.gun_place(weapon as usize, &p.pose) else {
            return Ok(0);
        };
        let held = (gr, gp);
        let (place, bone) = if in_hand { (1, "NPC R Hand [RHnd]") } else { (2, "NPC Spine2 [Spn2]") };
        let Some((q, t)) = r.retarget.attach_local(&p.pose, bone, held) else {
            return Ok(0);
        };
        unsafe {
            if !translation.is_null() {
                *translation = t.into();
            }
            if !rotation.is_null() {
                *rotation = SnakeQuat { x: q.x, y: q.y, z: q.z, w: q.w };
            }
        }
        Ok(place)
    })
}

/// The weapon in Snake's hands, if any: its index, and its transform
/// relative to Skyrim's right hand (`NPC R Hand [RHnd]`), in Skyrim axes
/// and units, for the mesh written by `mgsv skyrim-export`. Returns -1 when
/// no weapon is in the hands.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_held_weapon(
    r: *const SnakeRetarget,
    p: *const SnakePlayer,
    translation: *mut SnakeVec3,
    rotation: *mut SnakeQuat,
) -> i32 {
    guard(-1, || {
        // SAFETY: valid pointers or null.
        let (r, p) = unsafe {
            (
                r.as_ref().ok_or("null retarget")?,
                p.as_ref().ok_or("null player")?,
            )
        };
        let ctl = &p.controller;
        let Some(i) = ctl.current.filter(|_| ctl.weapon_in_hand()) else {
            return Ok(-1);
        };
        let Some((gr, gp, _)) = ctl.gun_place(i, &p.pose) else {
            return Ok(-1);
        };
        let held = (gr, gp);
        let Some((q, t)) = r.retarget.attach_local(&p.pose, "NPC R Hand [RHnd]", held) else {
            return Ok(-1);
        };
        unsafe {
            if !translation.is_null() {
                *translation = t.into();
            }
            if !rotation.is_null() {
                *rotation = SnakeQuat { x: q.x, y: q.y, z: q.z, w: q.w };
            }
        }
        Ok(i as i32)
    })
}

/// Local transforms (relative to each bone's Skyrim parent, Skyrim axes and
/// units) for the player's last pose. `rotations` gets one quaternion per
/// bone; `translations`/`has_translation` (optional) are set for bones whose
/// translation should change too (the COM: pelvis height). Returns the
/// number of bones.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snake_retarget_apply(
    r: *const SnakeRetarget,
    p: *const SnakePlayer,
    rotations: *mut SnakeQuat,
    translations: *mut SnakeVec3,
    has_translation: *mut u8,
    capacity: u32,
) -> u32 {
    guard(0, || {
        // SAFETY: valid pointers or null; arrays hold `capacity` entries.
        let (r, p) = unsafe {
            (
                r.as_ref().ok_or("null retarget")?,
                p.as_ref().ok_or("null player")?,
            )
        };
        let locals = r.retarget.apply(&p.pose);
        let n = locals.len().min(capacity as usize);
        for (i, l) in locals.iter().take(n).enumerate() {
            unsafe {
                if !rotations.is_null() {
                    *rotations.add(i) = SnakeQuat {
                        x: l.rotation.x,
                        y: l.rotation.y,
                        z: l.rotation.z,
                        w: l.rotation.w,
                    };
                }
                if !has_translation.is_null() {
                    *has_translation.add(i) = l.translation.is_some() as u8;
                }
                if let (Some(t), false) = (l.translation, translations.is_null()) {
                    *translations.add(i) = t.into();
                }
            }
        }
        Ok(locals.len() as u32)
    })
}
