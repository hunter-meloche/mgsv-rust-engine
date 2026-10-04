//! Weapons: stats from the game's own parameter tables, attaching the gun to
//! Snake's hand, the aim pose grid, and bullets.

use fox_anim::{HumanRig, Pose, Quat, RigFrame, Skeleton, Vec3};
use fox_data::lua_table::{LuaTable, LuaValue, parse_call_argument};
use fox_data::mtar::Clip;

use crate::world::{Collision, Hit};

/// Gameplay numbers for one weapon, read from
/// `/Assets/tpp/level_asset/weapon/ParameterTables/parts/EquipParameters.lua`.
#[derive(Clone, Debug)]
pub struct WeaponStats {
    pub weapon: String,
    pub receiver: String,
    pub rounds_per_minute: f32,
    pub full_auto: bool,
    pub magazine: u32,
    pub reserve: u32,
    pub bullet: String,
    /// Muzzle speed (m/s).
    pub bullet_speed: f32,
    /// Bullet drop acceleration (m/s^2); 9.8 for grenades, lower for sniper rounds.
    pub bullet_gravity: f32,
    /// The receiver's attack (`TppDamage.ATK_*`), keying the damage table.
    pub attack: String,
    /// Lethal damage per hit (MGSV scale: the MRS-4 does 600); 0 until
    /// [`WeaponStats::apply_damage_parameters`].
    pub lethal_damage: f32,
    /// Sleep (tranquilizer) damage per hit; the WU S.Pistol does 750 and no
    /// lethal damage.
    pub sleep_damage: f32,
}

impl WeaponStats {
    /// Look up a weapon (e.g. `WP_West_ar_010`, the AM MRS-4) in the
    /// `TppEquip.ReloadEquipParameterTables2{...}` tables.
    ///
    /// Row layouts used (0-based set indices):
    /// * `gunBasic`: weapon, receiver, barrel, magazine, stock, ...
    /// * `receiver`: id, attack id, base set, wobbling set, system set, sound set
    /// * `receiverParamSetsBase[i][0]`: rounds per minute
    /// * `receiverParamSetsSystem[i][2]`: trigger type
    /// * `magazine`: id, equip id, rounds, reserve, bullet
    /// * `bullet`: id, speed, speed, gravity, ...
    pub fn from_equip_parameters(src: &str, weapon: &str) -> Result<Self, String> {
        let t = parse_call_argument(src, "TppEquip.ReloadEquipParameterTables2")
            .map_err(|e| e.to_string())?;
        let table = |name: &str| -> Result<&LuaTable, String> {
            t.get(name)
                .and_then(LuaValue::as_table)
                .ok_or_else(|| format!("no {name} table"))
        };
        let ident = |r: &LuaTable, i: usize| {
            r.items
                .get(i)
                .and_then(LuaValue::as_ident)
                .map(str::to_string)
        };
        let num = |r: &LuaTable, i: usize| -> Result<f64, String> {
            r.items
                .get(i)
                .and_then(LuaValue::as_f64)
                .ok_or_else(|| format!("field {i} not a number"))
        };
        let nth = |name: &str, i: usize| -> Result<LuaTable, String> {
            table(name)?
                .rows()
                .nth(i)
                .cloned()
                .ok_or_else(|| format!("{name}[{i}] missing"))
        };

        let key = format!("TppEquip.{weapon}");
        let gun = table("gunBasic")?
            .row(&key)
            .ok_or_else(|| format!("{weapon} not in gunBasic"))?;
        let receiver = ident(gun, 1).ok_or("no receiver")?;
        let magazine = ident(gun, 3).ok_or("no magazine")?;
        let rc = table("receiver")?
            .row(&receiver)
            .ok_or_else(|| format!("{receiver} not found"))?;
        let base = nth("receiverParamSetsBase", num(rc, 2)? as usize)?;
        let system = nth("receiverParamSetsSystem", num(rc, 4)? as usize)?;
        let am = table("magazine")?
            .row(&magazine)
            .ok_or_else(|| format!("{magazine} not found"))?;
        let bullet = ident(am, 4).ok_or("no bullet")?;
        let bl = table("bullet")?
            .row(&bullet)
            .ok_or_else(|| format!("{bullet} not found"))?;
        let strip = |s: String| s.trim_start_matches("TppEquip.").to_string();
        Ok(Self {
            weapon: weapon.to_string(),
            receiver: strip(receiver),
            rounds_per_minute: num(&base, 0)? as f32,
            full_auto: ident(&system, 2).is_some_and(|t| t.ends_with("FULLAUTO")),
            magazine: num(am, 2)? as u32,
            reserve: num(am, 3)? as u32,
            bullet: strip(bullet),
            bullet_speed: num(bl, 1)? as f32,
            bullet_gravity: num(bl, 3)? as f32,
            attack: ident(rc, 1)
                .map(|a| a.trim_start_matches("TppDamage.").to_string())
                .unwrap_or_default(),
            lethal_damage: 0.0,
            sleep_damage: 0.0,
        })
    }

    /// Fill in the damage of this weapon's attack from the
    /// `TppDamage.ReloadDamageParameter{DamageParameter={...}}` table, whose
    /// rows start `attack, lethal damage, ?, ?, sleep damage, ...`.
    pub fn apply_damage_parameters(&mut self, src: &str) -> Result<(), String> {
        let t = parse_call_argument(src, "TppDamage.ReloadDamageParameter").map_err(|e| e.to_string())?;
        let rows = t
            .get("DamageParameter")
            .and_then(LuaValue::as_table)
            .ok_or("no DamageParameter table")?;
        let key = format!("TppDamage.{}", self.attack);
        let row = rows.row(&key).ok_or_else(|| format!("{} not in DamageParameter", self.attack))?;
        let num = |i: usize| -> Result<f32, String> {
            row.items
                .get(i)
                .and_then(LuaValue::as_f64)
                .map(|v| v as f32)
                .ok_or_else(|| format!("{}: field {i} not a number", self.attack))
        };
        self.lethal_damage = num(1)?;
        self.sleep_damage = num(4)?;
        Ok(())
    }
}

/// One aim pose grid clip (e.g. `snapasr_s_fre_pose_25`): one pose per
/// frame, row-major from the top-left, rows by pitch (highest first) and
/// columns by yaw (leftmost first). The angles are measured from the poses
/// when the weapon is calibrated, so any grid layout works (rifle stand /
/// crouch 5 x 5 at 45 / 37.5 degrees; prone 9 x 4 covering +-180 degrees).
#[derive(Clone, Debug)]
pub struct AimGrid {
    pub clip: String,
    pub cols: usize,
    pub rows: usize,
    /// Yaw of each column (radians, decreasing: left to right).
    pub col_yaw: Vec<f32>,
    /// Pitch of each row (radians, decreasing: top to bottom).
    pub row_pitch: Vec<f32>,
}

/// Fractional index of `v` in a decreasing sequence (clamped).
fn index_in(seq: &[f32], v: f32) -> f32 {
    if seq.len() < 2 || v >= seq[0] {
        return 0.0;
    }
    for i in 1..seq.len() {
        if v >= seq[i] {
            let span = seq[i - 1] - seq[i];
            return (i - 1) as f32
                + if span.abs() > 1e-6 {
                    (seq[i - 1] - v) / span
                } else {
                    0.0
                };
        }
    }
    (seq.len() - 1) as f32
}

/// Clips and attachment for one weapon on Snake's rig.
#[derive(Clone, Debug)]
pub struct WeaponRig {
    /// Aim grids for stand, crouch and prone (prone may be missing).
    pub grids: [Option<AimGrid>; 3],
    pub reload: String,
    /// Played on the upper body for each shot (recoil / cycling), if any.
    pub fire: Option<String>,
    /// Skeleton bone the gun hangs from and the body's connection point on it.
    pub hand_bone: usize,
    pub hand_offset: (Quat, Vec3),
    /// Gun frame relative to the hand connection point.
    pub hand_to_gun: Quat,
    /// Muzzle position in the gun frame (from the gun's `.fcnp`).
    pub muzzle: Vec3,
}

/// Names of a weapon's clips.
#[derive(Clone, Debug)]
pub struct WeaponClips<'a> {
    /// Stand, crouch and prone aim grids.
    pub grids: [Option<&'a str>; 3],
    pub reload: &'a str,
    pub fire: Option<&'a str>,
}

impl WeaponRig {
    /// Calibrate a weapon on Snake's rig. `hand_offset` is the body's
    /// `CNP_RIGHT_HAND` on `hand_bone`. The gun-to-hand rotation comes from
    /// the standing grid's centre pose, where the weapon aims straight ahead
    /// and level (verified on the rifle: every other pose then lands on exact
    /// 45 / 37.5 degree steps with the off hand on the gun's fore grip); the
    /// grids' angles are then measured with it.
    pub fn calibrate(
        rig: &HumanRig,
        skel: &Skeleton,
        clips: &crate::ClipLibrary,
        names: WeaponClips,
        hand_bone: usize,
        hand_offset: (Quat, Vec3),
        muzzle: Vec3,
    ) -> Result<Self, String> {
        let stand_name = names.grids[0].ok_or("a standing aim grid is required")?;
        let stand = clips
            .get(stand_name)
            .ok_or_else(|| format!("missing clip {stand_name}"))?;
        let centre = rig.pose(skel, &stand, (stand.frame_count / 2) as f32)?;
        let hand_to_gun = (centre.rotations[hand_bone] * hand_offset.0).inverse();
        let mut this = Self {
            grids: [None, None, None],
            reload: names.reload.to_string(),
            fire: names.fire.map(str::to_string),
            hand_bone,
            hand_offset,
            hand_to_gun,
            muzzle,
        };
        for (slot, name) in names.grids.iter().enumerate() {
            let Some(name) = name else { continue };
            let clip = clips
                .get(name)
                .ok_or_else(|| format!("missing clip {name}"))?;
            this.grids[slot] = Some(this.measure(rig, skel, name, &clip)?);
        }
        Ok(this)
    }

    /// Measure a grid's layout and angles from its poses.
    fn measure(
        &self,
        rig: &HumanRig,
        skel: &Skeleton,
        name: &str,
        clip: &Clip,
    ) -> Result<AimGrid, String> {
        let n = clip.frame_count as usize + 1;
        let mut dirs = Vec::with_capacity(n);
        for f in 0..n {
            let (r, _) = self.gun_transform(&rig.pose(skel, clip, f as f32)?);
            let d = r * Vec3::Z;
            dirs.push((d.x.atan2(d.z), d.y.clamp(-1.0, 1.0).asin()));
        }
        // A row ends where the pitch jumps.
        let cols = (1..n)
            .find(|&i| (dirs[i].1 - dirs[0].1).abs() > 12f32.to_radians())
            .unwrap_or(n);
        let rows = n / cols;
        if rows * cols != n || cols < 2 || rows < 2 {
            return Err(format!("{name}: cannot infer a grid from {n} poses"));
        }
        let tau = std::f32::consts::TAU;
        let mut col_yaw: Vec<f32> = (0..cols)
            .map(|c| (0..rows).map(|r| dirs[r * cols + c].0).sum::<f32>() / rows as f32)
            .collect();
        // Unwrap outward from the column nearest straight ahead so yaws
        // decrease left to right (prone grids span +180..-180).
        let mid = (0..cols)
            .min_by(|&a, &b| col_yaw[a].abs().total_cmp(&col_yaw[b].abs()))
            .unwrap();
        for c in (0..mid).rev() {
            while col_yaw[c] <= col_yaw[c + 1] {
                col_yaw[c] += tau;
            }
        }
        for c in mid + 1..cols {
            while col_yaw[c] >= col_yaw[c - 1] {
                col_yaw[c] -= tau;
            }
        }
        let row_pitch = (0..rows)
            .map(|r| (0..cols).map(|c| dirs[r * cols + c].1).sum::<f32>() / cols as f32)
            .collect();
        Ok(AimGrid {
            clip: name.to_string(),
            cols,
            rows,
            col_yaw,
            row_pitch,
        })
    }

    /// Gun transform in the pose's space (root space for controller poses).
    pub fn gun_transform(&self, pose: &Pose) -> (Quat, Vec3) {
        let (hr, hp) = (
            pose.rotations[self.hand_bone],
            pose.positions[self.hand_bone],
        );
        let rot = hr * self.hand_offset.0 * self.hand_to_gun;
        (rot, hp + hr * self.hand_offset.1)
    }

    /// Bilinearly blended aim pose for `yaw` (left positive) and `pitch`
    /// (up positive), radians relative to the body.
    pub fn aim_frame(
        &self,
        rig: &HumanRig,
        grid: &AimGrid,
        clip: &Clip,
        yaw: f32,
        pitch: f32,
    ) -> Result<RigFrame, String> {
        let c = index_in(&grid.col_yaw, yaw);
        let r = index_in(&grid.row_pitch, pitch);
        let (c0, r0) = (c.floor() as usize, r.floor() as usize);
        let (c1, r1) = ((c0 + 1).min(grid.cols - 1), (r0 + 1).min(grid.rows - 1));
        let (tc, tr) = (c - c0 as f32, r - r0 as f32);
        let at = |row: usize, col: usize| rig.sample_frame(clip, (row * grid.cols + col) as f32);
        let top = rig.blend(&at(r0, c0)?, &at(r0, c1)?, tc);
        let bottom = rig.blend(&at(r1, c0)?, &at(r1, c1)?, tc);
        Ok(rig.blend(&top, &bottom, tr))
    }
}

#[derive(Clone, Debug)]
pub struct Bullet {
    pub position: Vec3,
    pub velocity: Vec3,
    pub gravity: f32,
    pub age: f32,
    /// Position one step ago (for drawing tracers).
    pub previous: Vec3,
}

/// Bullets in flight, stepped with gravity against a [`Collision`] world.
#[derive(Default)]
pub struct Ballistics {
    pub bullets: Vec<Bullet>,
    /// Recent impacts with their age (seconds).
    pub impacts: Vec<(Hit, f32)>,
}

impl Ballistics {
    pub fn fire(&mut self, origin: Vec3, direction: Vec3, speed: f32, gravity: f32) {
        self.bullets.push(Bullet {
            position: origin,
            velocity: direction.normalize() * speed,
            gravity,
            age: 0.0,
            previous: origin,
        });
    }

    /// Advance bullets; returns this step's new hits.
    pub fn step(&mut self, dt: f32, world: &dyn Collision) -> Vec<Hit> {
        let mut hits = Vec::new();
        self.bullets.retain_mut(|b| {
            b.previous = b.position;
            b.velocity.y -= b.gravity * dt;
            let next = b.position + b.velocity * dt;
            b.age += dt;
            if let Some(hit) = world.raycast(b.position, next) {
                hits.push(hit);
                return false;
            }
            b.position = next;
            b.age < 3.0
        });
        for (_, age) in &mut self.impacts {
            *age += dt;
        }
        self.impacts.retain(|(_, age)| *age < 6.0);
        self.impacts.extend(hits.iter().map(|h| (*h, 0.0)));
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_index_interpolates_decreasing_angles() {
        let seq = [1.0, 0.5, 0.0, -0.5, -1.0];
        assert_eq!(index_in(&seq, 2.0), 0.0);
        assert_eq!(index_in(&seq, 0.25), 1.5);
        assert_eq!(index_in(&seq, -0.75), 3.5);
        assert_eq!(index_in(&seq, -5.0), 4.0);
    }

    #[test]
    fn stats_follow_the_table_links() {
        // Same shape as EquipParameters.lua, trimmed to one weapon.
        let src = "this={}TppEquip.ReloadEquipParameterTables2{\
            gunBasic={1,{TppEquip.WP_X,TppEquip.RC_1,TppEquip.BA_1,TppEquip.AM_1,TppEquip.SK_None,3}},\
            receiverParamSetsBase={{300,1},{720,46}},\
            receiverParamSetsSystem={{TppEquip.EQP_TYPE_Handgun,0,TppEquip.TRIGGER_SEMIAUTO},{TppEquip.EQP_TYPE_Assault,0,TppEquip.TRIGGER_FULLAUTO}},\
            receiver={1,{TppEquip.RC_1,TppDamage.ATK_1,1,0,1,13}},\
            magazine={1,{TppEquip.AM_1,TppEquip.EQP_AM_1,30,210,TppEquip.BL_Rf556x45mm}},\
            bullet={0,{TppEquip.BL_Rf556x45mm,450,450,18,7}}}";
        let s = WeaponStats::from_equip_parameters(src, "WP_X").unwrap();
        assert_eq!(s.rounds_per_minute, 720.0);
        assert!(s.full_auto);
        assert_eq!((s.magazine, s.reserve), (30, 210));
        assert_eq!((s.bullet_speed, s.bullet_gravity), (450.0, 18.0));
    }

    #[test]
    fn bullets_drop_and_hit_the_ground() {
        let mut b = Ballistics::default();
        b.fire(Vec3::new(0.0, 1.5, 0.0), Vec3::Z, 450.0, 18.0);
        let ground = crate::world::FlatGround { height: 0.0 };
        for _ in 0..120 {
            b.step(1.0 / 60.0, &ground);
        }
        assert!(b.bullets.is_empty());
        let (hit, _) = b.impacts[0];
        // Drop 1.5 m at 18 m/s^2 takes ~0.41 s: ~183 m downrange.
        assert!((hit.point.z - 183.0).abs() < 5.0, "{:?}", hit.point);
    }
}
