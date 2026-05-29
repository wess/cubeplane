//! World simulation: random-tick growth, fluid flow, fire spread and redstone.
//!
//! These run from the game loop against the blocks in loaded chunks. They use
//! the generic block-property helpers (`with_prop`, `prop_index`) so they work
//! off real block-state data rather than hard-coded ids.

use std::sync::Arc;

use rand::Rng;
use uuid::Uuid;

use cubeplane_world::block;

use crate::clientbound as cb;
use crate::state::Shared;

/// Maximum world-coordinate columns to scan per pass (bounds cost).
const RANDOM_TICKS_PER_CHUNK: usize = 12;

/// Set a block in the world and tell every player.
fn set(shared: &Arc<Shared>, x: i32, y: i32, z: i32, state: u16) {
    shared.world.lock().unwrap().set_block(x, y, z, state);
    shared.broadcast(cb::block_update(x, y, z, state));
}

fn get(shared: &Arc<Shared>, x: i32, y: i32, z: i32) -> u16 {
    shared.world.lock().unwrap().get_block(x, y, z)
}

// ---------------------------------------------------------------------------
// Random-tick growth
// ---------------------------------------------------------------------------

/// Random-tick a sample of blocks in loaded chunks: grow crops and saplings,
/// spread grass. Called a few times per second.
pub fn random_tick(shared: &Arc<Shared>) {
    let coords = shared.world.lock().unwrap().loaded_coords();
    let mut rng = rand::thread_rng();
    for (cx, cz) in coords {
        for _ in 0..RANDOM_TICKS_PER_CHUNK {
            let x = cx * 16 + rng.gen_range(0..16);
            let z = cz * 16 + rng.gen_range(0..16);
            let y = rng.gen_range(-32..96);
            tick_block(shared, &mut rng, x, y, z);
        }
    }
}

fn tick_block(shared: &Arc<Shared>, rng: &mut impl Rng, x: i32, y: i32, z: i32) {
    let state = get(shared, x, y, z);
    let name = block::name_of(state);

    // Crops with an `age` property advance toward maturity.
    if is_crop(name) {
        if let (Some(age), Some(max)) = (block::prop_index(state, "age"), block::prop_values(state, "age")) {
            if age + 1 < max as u32 && rng.gen_bool(0.5) {
                set(shared, x, y, z, block::with_prop(state, "age", age + 1));
            }
        }
        return;
    }

    // Saplings grow into a small tree.
    if name.ends_with("_sapling") && rng.gen_bool(0.25) {
        grow_tree(shared, x, y, z, name);
        return;
    }

    // Grass spreads onto adjacent dirt with sky access.
    if name == "grass_block" {
        let (dx, dz) = (rng.gen_range(-1..=1), rng.gen_range(-1..=1));
        let (nx, nz) = (x + dx, z + dz);
        if block::name_of(get(shared, nx, y, nz)) == "dirt"
            && block::is_air(get(shared, nx, y + 1, nz))
        {
            set(shared, nx, y, nz, block::state_by_name("grass_block").unwrap());
        }
    }
}

fn is_crop(name: &str) -> bool {
    matches!(name, "wheat" | "carrots" | "potatoes" | "beetroots" | "nether_wart")
}

/// Replace a sapling with a small log + leaves tree.
fn grow_tree(shared: &Arc<Shared>, x: i32, y: i32, z: i32, sapling: &str) {
    let species = sapling.strip_suffix("_sapling").unwrap_or("oak");
    let log = block::state_by_name(&format!("{species}_log")).unwrap_or(block::OAK_LOG);
    let leaves = block::state_by_name(&format!("{species}_leaves")).unwrap_or(block::OAK_LEAVES);

    let height = 4;
    for i in 0..height {
        set(shared, x, y + i, z, log);
    }
    // A 3×3 (and a small cap) canopy of leaves around the top.
    for dy in (height - 2)..=(height + 1) {
        let r = if dy >= height { 1 } else { 2 };
        for dx in -r..=r {
            for dz in -r..=r {
                if dx == 0 && dz == 0 && dy < height {
                    continue; // trunk
                }
                let (lx, ly, lz) = (x + dx, y + dy, z + dz);
                if block::is_air(get(shared, lx, ly, lz)) {
                    set(shared, lx, ly, lz, leaves);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fluid flow (water & lava)
// ---------------------------------------------------------------------------

/// Process queued fluid cells: each fluid flows down into air, then outward
/// losing one level up to its range. New fluid cells reschedule themselves, so
/// flow continues over subsequent ticks. Queue-driven, so it costs nothing when
/// no fluids are changing.
pub fn fluid_tick(shared: &Arc<Shared>) {
    let cells = shared.drain_fluid(2048);
    for (x, y, z) in cells {
        let state = get(shared, x, y, z);
        let name = block::name_of(state);
        if name != "water" && name != "lava" {
            continue;
        }
        let level = block::prop_index(state, "level").unwrap_or(0);
        let range: u32 = if name == "water" { 7 } else { 3 };

        // Infinite water source: a flowing cell flanked by ≥2 source blocks
        // becomes a source itself.
        if name == "water" && level > 0 {
            let sources = [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
                .iter()
                .filter(|(nx, nz)| {
                    let s = get(shared, *nx, y, *nz);
                    block::name_of(s) == "water" && block::prop_index(s, "level") == Some(0)
                })
                .count();
            if sources >= 2 {
                set(shared, x, y, z, state_with_level("water", 0));
                continue;
            }
        }
        if level >= range {
            continue;
        }

        // Flow straight down as a fresh falling column.
        if block::is_air(get(shared, x, y - 1, z)) {
            place_fluid(shared, x, y - 1, z, name, 0);
            continue;
        }
        // Otherwise spread outward at one higher level.
        for (nx, nz) in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
            if block::is_air(get(shared, nx, y, nz)) {
                place_fluid(shared, nx, y, nz, name, level + 1);
            }
        }
    }
}

/// Place flowing fluid at a cell — unless it meets the opposite fluid, in which
/// case it solidifies (water+lava → stone/cobblestone/obsidian).
fn place_fluid(shared: &Arc<Shared>, x: i32, y: i32, z: i32, name: &str, level: u32) {
    let opposite = if name == "water" { "lava" } else { "water" };
    let mut opp_source = false;
    let mut opp_any = false;
    for (nx, ny, nz) in [(x + 1, y, z), (x - 1, y, z), (x, y + 1, z), (x, y - 1, z), (x, y, z + 1), (x, y, z - 1)] {
        let s = get(shared, nx, ny, nz);
        if block::name_of(s) == opposite {
            opp_any = true;
            if block::prop_index(s, "level") == Some(0) {
                opp_source = true;
            }
        }
    }
    if opp_any {
        // Lava meeting water: lava source → obsidian, flowing lava → cobblestone.
        // Water flowing onto lava → stone.
        let result = if name == "water" {
            if opp_source { "obsidian" } else { "cobblestone" }
        } else {
            "stone"
        };
        if let Some(state) = block::state_by_name(result) {
            set(shared, x, y, z, state);
        }
        return;
    }
    set(shared, x, y, z, state_with_level(name, level));
    shared.schedule_fluid(x, y, z);
}

fn state_with_level(fluid: &str, level: u32) -> u16 {
    let base = block::state_by_name(fluid).unwrap_or(block::WATER);
    block::with_prop(base, "level", level)
}

// ---------------------------------------------------------------------------
// Gravity (falling sand / gravel)
// ---------------------------------------------------------------------------

fn is_gravity(name: &str) -> bool {
    name == "sand" || name == "red_sand" || name == "gravel" || name.ends_with("concrete_powder")
}

/// If a gravity block at `(x,y,z)` is unsupported, drop it to the lowest air,
/// then let whatever is above fall in turn.
pub fn gravity_check(shared: &Arc<Shared>, x: i32, y: i32, z: i32) {
    let state = get(shared, x, y, z);
    if !is_gravity(block::name_of(state)) {
        return;
    }
    let mut ny = y;
    while ny > cubeplane_world::chunk::MIN_Y + 1 && block::is_air(get(shared, x, ny - 1, z)) {
        ny -= 1;
    }
    if ny < y {
        set(shared, x, y, z, block::AIR);
        set(shared, x, ny, z, state);
        // The block that was above the old position may now fall too.
        gravity_check(shared, x, y + 1, z);
    }
}

// ---------------------------------------------------------------------------
// Fire
// ---------------------------------------------------------------------------

/// Fire spreads to flammable neighbours and burns out over time.
pub fn fire_tick(shared: &Arc<Shared>) {
    let coords = shared.world.lock().unwrap().loaded_coords();
    let mut rng = rand::thread_rng();
    let mut extinguish = Vec::new();
    let mut ignite = Vec::new();

    for (cx, cz) in coords {
        for _ in 0..6 {
            let x = cx * 16 + rng.gen_range(0..16);
            let z = cz * 16 + rng.gen_range(0..16);
            let y = rng.gen_range(-32..96);
            if block::name_of(get(shared, x, y, z)) != "fire" {
                continue;
            }
            // Spread to a random flammable neighbour.
            let (nx, ny, nz) = (x + rng.gen_range(-1..=1), y + rng.gen_range(-1..=1), z + rng.gen_range(-1..=1));
            if is_flammable(block::name_of(get(shared, nx, ny, nz))) && rng.gen_bool(0.3) {
                ignite.push((nx, ny, nz));
            }
            // Eventually burn out.
            if rng.gen_bool(0.25) {
                extinguish.push((x, y, z));
            }
        }
    }

    let fire = block::state_by_name("fire").unwrap_or(block::AIR);
    for (x, y, z) in ignite {
        set(shared, x, y, z, fire);
    }
    for (x, y, z) in extinguish {
        set(shared, x, y, z, block::AIR);
    }
}

fn is_flammable(name: &str) -> bool {
    name.ends_with("_log")
        || name.ends_with("_planks")
        || name.ends_with("_leaves")
        || name.ends_with("_wool")
        || name == "oak_log"
}

// ---------------------------------------------------------------------------
// TNT
// ---------------------------------------------------------------------------

/// Prime the TNT block at `(x,y,z)`: clear it, spawn a flashing TNT entity, and
/// start an 80-tick fuse.
pub fn ignite_tnt(shared: &Arc<Shared>, x: i32, y: i32, z: i32) {
    set(shared, x, y, z, block::AIR);
    let eid = shared.next_entity_id();
    let (fx, fy, fz) = (x as f64 + 0.5, y as f64, z as f64 + 0.5);
    shared.broadcast(cb::spawn_entity(
        eid,
        Uuid::new_v4(),
        crate::entity::TNT,
        fx,
        fy,
        fz,
        0.0,
        0.0,
        0.0,
        0,
        (0, 0, 0),
    ));
    shared.add_tnt(eid, fx, fy, fz, 80);
}

/// Detonate any TNT whose fuse has expired.
pub fn tnt_tick(shared: &Arc<Shared>) {
    for (eid, x, y, z) in shared.tick_tnt() {
        shared.broadcast(cb::remove_entities(&[eid]));
        crate::mobs::explode(shared, x, y, z, 4.0);
    }
}

// ---------------------------------------------------------------------------
// Redstone
// ---------------------------------------------------------------------------

/// Whether a block participates in redstone (so we know when to recompute).
pub fn is_redstone(name: &str) -> bool {
    is_redstone_source(name) || is_actuator(name) || is_logic(name) || is_piston(name)
}

/// A piston base block.
fn is_piston(name: &str) -> bool {
    matches!(name, "piston" | "sticky_piston")
}

/// A block that emits redstone power.
fn is_redstone_source(name: &str) -> bool {
    matches!(name, "redstone_wire" | "redstone_block" | "redstone_torch" | "redstone_wall_torch" | "lever")
        || name.ends_with("_pressure_plate")
        || name.ends_with("_button")
}

/// A block that reacts to redstone power: lamps light, doors/gates open.
fn is_actuator(name: &str) -> bool {
    name == "redstone_lamp"
        || name.ends_with("_door")
        || name.ends_with("_trapdoor")
        || name.ends_with("_fence_gate")
}

/// A directional logic component (repeater/comparator) that we re-evaluate
/// during a flood and toggle after a delay.
fn is_logic(name: &str) -> bool {
    matches!(name, "repeater" | "comparator")
}

/// Recompute redstone power in a box around a change: flood power from sources
/// through wire (−1 per step) and light/unlight lamps accordingly. Bounded and
/// local, so it's cheap to run on each redstone-related block change.
pub fn redstone_update(shared: &Arc<Shared>, ox: i32, oy: i32, oz: i32) {
    use std::collections::{HashMap, VecDeque};
    const R: i32 = 12;

    let mut world = shared.world.lock().unwrap();
    // Snapshot redstone-relevant blocks in the box.
    let mut wires: HashMap<(i32, i32, i32), u16> = HashMap::new();
    let mut actuators: Vec<(i32, i32, i32, u16)> = Vec::new();
    let mut repeaters: Vec<(i32, i32, i32, u16)> = Vec::new();
    let mut pistons: Vec<(i32, i32, i32, u16)> = Vec::new();
    let mut power: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut queue: VecDeque<(i32, i32, i32, u32)> = VecDeque::new();

    for dx in -R..=R {
        for dy in -R..=R {
            for dz in -R..=R {
                let (x, y, z) = (ox + dx, oy + dy, oz + dz);
                let s = world.get_block(x, y, z);
                let name = block::name_of(s);
                if name == "redstone_wire" {
                    wires.insert((x, y, z), s);
                } else if is_actuator(name) {
                    actuators.push((x, y, z, s));
                } else if matches!(name, "repeater" | "comparator") {
                    repeaters.push((x, y, z, s));
                } else if is_piston(name) {
                    pistons.push((x, y, z, s));
                } else if name == "redstone_block" {
                    queue.push_back((x, y, z, 15));
                } else if matches!(name, "redstone_torch" | "redstone_wall_torch") {
                    if block::prop_index(s, "lit") == Some(0) {
                        queue.push_back((x, y, z, 15));
                    }
                } else if (name == "lever" || name.ends_with("_pressure_plate") || name.ends_with("_button"))
                    && block::prop_index(s, "powered") == Some(0)
                {
                    queue.push_back((x, y, z, 15));
                }
            }
        }
    }

    // A powered repeater/comparator injects full power into the block it faces.
    for (x, y, z, s) in &repeaters {
        if block::prop_index(*s, "powered") == Some(0) {
            if let Some(facing) = block::prop_index(*s, "facing") {
                let (dx, dz) = dir4(facing);
                let front = (x + dx, *y, z + dz);
                power.insert(front, 15);
                queue.push_back((front.0, front.1, front.2, 15));
            }
        }
    }

    // Flood power into wires (−1 per wire step).
    while let Some((x, y, z, p)) = queue.pop_front() {
        for (nx, ny, nz) in neighbors6(x, y, z) {
            if wires.contains_key(&(nx, ny, nz)) {
                let next = p.saturating_sub(1);
                let entry = power.entry((nx, ny, nz)).or_insert(0);
                if next > *entry {
                    *entry = next;
                    if next > 1 {
                        queue.push_back((nx, ny, nz, next));
                    }
                }
            }
        }
    }

    // Apply wire power and lamp lit changes.
    let mut changes: Vec<(i32, i32, i32, u16)> = Vec::new();
    for (pos, state) in &wires {
        let p = power.get(pos).copied().unwrap_or(0);
        let want = block::with_prop(*state, "power", p);
        if want != *state {
            changes.push((pos.0, pos.1, pos.2, want));
        }
    }
    for (x, y, z, state) in actuators {
        let name = block::name_of(state);
        // Power can also be injected directly onto the actuator's own cell
        // (e.g. a repeater pointing straight into a lamp).
        let own = power.get(&(x, y, z)).copied().unwrap_or(0) > 0;
        // A door is powered if power reaches either of its two halves.
        let powered = own
            || if name.ends_with("_door") {
                let other = if block::prop_index(state, "half") == Some(0) { y - 1 } else { y + 1 };
                powered_at(&power, &mut world, x, y, z) || powered_at(&power, &mut world, x, other, z)
            } else {
                powered_at(&power, &mut world, x, y, z)
            };
        // Lamps drive "lit" (0 = on); doors/gates drive "open" (0 = open).
        let prop = if name == "redstone_lamp" { "lit" } else { "open" };
        let want = block::with_prop(state, prop, if powered { 0 } else { 1 });
        if want != state {
            changes.push((x, y, z, want));
        }
    }
    for (x, y, z, s) in &changes {
        world.set_block(*x, *y, *z, *s);
    }

    // Re-evaluate repeaters/comparators: schedule output toggles after delay.
    for (x, y, z, state) in &repeaters {
        let (x, y, z) = (*x, *y, *z);
        let name = block::name_of(*state);
        let facing = match block::prop_index(*state, "facing") {
            Some(f) => f,
            None => continue,
        };
        let (dx, dz) = dir4(facing);
        // Input comes from directly behind (opposite the facing/output side).
        let back = (x - dx, y, z - dz);
        let back_state = world.get_block(back.0, back.1, back.2);
        let back_powered = power.get(&back).copied().unwrap_or(0) > 0
            || is_source(&back_state)
            || repeater_feeds(&mut world, back, (x, y, z));
        let desired_on = if name == "comparator" {
            // Comparator (compare mode): pass the back signal if no stronger
            // signal sits on either side.
            let sides = comparator_side_power(&power, &mut world, x, y, z, facing);
            back_powered && sides == 0
        } else {
            back_powered
        };
        let current_on = block::prop_index(*state, "powered") == Some(0);
        if desired_on != current_on && !shared.change_pending(0, x, y, z) {
            let want = block::with_prop(*state, "powered", if desired_on { 0 } else { 1 });
            // Repeater delay is 1–4 (2 game ticks each); comparators take 2.
            let ticks = if name == "comparator" {
                2
            } else {
                ((block::prop_index(*state, "delay").unwrap_or(0) + 1) * 2) as u32
            };
            shared.schedule_block(0, x, y, z, want, ticks);
        }
    }

    // Pistons extend when powered and retract when not.
    let mut piston_changes: Vec<(i32, i32, i32, u16)> = Vec::new();
    for (x, y, z, state) in &pistons {
        let (x, y, z) = (*x, *y, *z);
        let powered = powered_at(&power, &mut world, x, y, z)
            || power.get(&(x, y, z)).copied().unwrap_or(0) > 0;
        let extended = block::prop_index(*state, "extended") == Some(0);
        if powered && !extended {
            extend_piston(&mut world, x, y, z, *state, &mut piston_changes);
        } else if !powered && extended {
            retract_piston(&mut world, x, y, z, *state, &mut piston_changes);
        }
    }

    drop(world);
    for (x, y, z, s) in changes {
        shared.broadcast(cb::block_update(x, y, z, s));
    }
    for (x, y, z, s) in piston_changes {
        shared.broadcast(cb::block_update(x, y, z, s));
    }
}

/// 6-value facing direction `[down(-Y), up(+Y), north(-Z), south(+Z), west(-X), east(+X)]`.
fn dir6(idx: u32) -> (i32, i32, i32) {
    match idx {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        _ => (1, 0, 0),
    }
}

/// Blocks a piston can shove. Excludes air, unmovable blocks and block
/// entities (whose contents we don't relocate).
fn is_pushable(state: u16) -> bool {
    if block::is_air(state) {
        return false;
    }
    let n = block::name_of(state);
    !matches!(
        n,
        "bedrock"
            | "obsidian"
            | "crying_obsidian"
            | "respawn_anchor"
            | "reinforced_deepslate"
            | "barrier"
            | "end_portal_frame"
            | "piston"
            | "sticky_piston"
            | "piston_head"
            | "moving_piston"
            | "chest"
            | "trapped_chest"
            | "ender_chest"
            | "furnace"
            | "blast_furnace"
            | "smoker"
            | "brewing_stand"
            | "hopper"
            | "dispenser"
            | "dropper"
            | "beacon"
            | "spawner"
            | "enchanting_table"
            | "anvil"
    ) && !n.ends_with("_sign")
        && !n.ends_with("_shulker_box")
}

/// Try to extend a piston: shove the column in front forward one block and place
/// the piston head. Aborts (does nothing) if the column is blocked or too long.
fn extend_piston(world: &mut cubeplane_world::World, x: i32, y: i32, z: i32, state: u16, out: &mut Vec<(i32, i32, i32, u16)>) {
    let facing = match block::prop_index(state, "facing") {
        Some(f) => f,
        None => return,
    };
    let (dx, dy, dz) = dir6(facing);
    let sticky = block::name_of(state) == "sticky_piston";

    // Collect the contiguous run of pushable blocks ahead until we hit air.
    let mut col: Vec<(i32, i32, i32, u16)> = Vec::new();
    let (mut cx, mut cy, mut cz) = (x + dx, y + dy, z + dz);
    loop {
        let b = world.get_block(cx, cy, cz);
        if block::is_air(b) {
            break;
        }
        if !is_pushable(b) || col.len() >= 12 {
            return; // blocked: the piston can't extend
        }
        col.push((cx, cy, cz, b));
        cx += dx;
        cy += dy;
        cz += dz;
    }

    // Shift the column forward from the far end so nothing is overwritten.
    for (bx, by, bz, b) in col.iter().rev() {
        world.set_block(bx + dx, by + dy, bz + dz, *b);
        out.push((bx + dx, by + dy, bz + dz, *b));
    }

    // Place the head and mark the base extended.
    let head_default = block::state_by_name("piston_head").unwrap_or(0);
    let mut head = block::with_prop(head_default, "facing", facing);
    head = block::with_prop(head, "type", if sticky { 1 } else { 0 });
    world.set_block(x + dx, y + dy, z + dz, head);
    out.push((x + dx, y + dy, z + dz, head));

    let base = block::with_prop(state, "extended", 0);
    world.set_block(x, y, z, base);
    out.push((x, y, z, base));
}

/// Retract a piston: remove its head and, if sticky, pull the block in front back.
fn retract_piston(world: &mut cubeplane_world::World, x: i32, y: i32, z: i32, state: u16, out: &mut Vec<(i32, i32, i32, u16)>) {
    let facing = match block::prop_index(state, "facing") {
        Some(f) => f,
        None => return,
    };
    let (dx, dy, dz) = dir6(facing);
    let sticky = block::name_of(state) == "sticky_piston";
    let air = block::state_by_name("air").unwrap_or(0);
    let head_pos = (x + dx, y + dy, z + dz);

    // Remove the head if it's there.
    if block::name_of(world.get_block(head_pos.0, head_pos.1, head_pos.2)) == "piston_head" {
        world.set_block(head_pos.0, head_pos.1, head_pos.2, air);
        out.push((head_pos.0, head_pos.1, head_pos.2, air));
    }

    // A sticky piston drags the block beyond its head back into the head's space.
    if sticky {
        let pull = (head_pos.0 + dx, head_pos.1 + dy, head_pos.2 + dz);
        let pb = world.get_block(pull.0, pull.1, pull.2);
        if is_pushable(pb) {
            world.set_block(head_pos.0, head_pos.1, head_pos.2, pb);
            out.push((head_pos.0, head_pos.1, head_pos.2, pb));
            world.set_block(pull.0, pull.1, pull.2, air);
            out.push((pull.0, pull.1, pull.2, air));
        }
    }

    let base = block::with_prop(state, "extended", 1);
    world.set_block(x, y, z, base);
    out.push((x, y, z, base));
}

/// 4-value facing direction `[north(-Z), south(+Z), west(-X), east(+X)]`.
fn dir4(idx: u32) -> (i32, i32) {
    match idx {
        0 => (0, -1),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (1, 0),
    }
}

/// Whether the block at `from` is a powered repeater/comparator whose output
/// faces `into`.
fn repeater_feeds(world: &mut cubeplane_world::World, from: (i32, i32, i32), into: (i32, i32, i32)) -> bool {
    let s = world.get_block(from.0, from.1, from.2);
    if !matches!(block::name_of(s), "repeater" | "comparator") {
        return false;
    }
    if block::prop_index(s, "powered") != Some(0) {
        return false;
    }
    if let Some(f) = block::prop_index(s, "facing") {
        let (dx, dz) = dir4(f);
        return (from.0 + dx, from.1, from.2 + dz) == into;
    }
    false
}

/// Strongest redstone signal entering a comparator from its two sides.
fn comparator_side_power(
    power: &std::collections::HashMap<(i32, i32, i32), u32>,
    world: &mut cubeplane_world::World,
    x: i32,
    y: i32,
    z: i32,
    facing: u32,
) -> u32 {
    // Sides are the two horizontal directions perpendicular to the facing axis:
    // facing north/south (0/1) → sides west/east (2/3), and vice versa.
    let sides: [u32; 2] = if facing < 2 { [2, 3] } else { [0, 1] };
    let mut max = 0;
    for f in sides {
        let (dx, dz) = dir4(f);
        let pos = (x + dx, y, z + dz);
        let p = power.get(&pos).copied().unwrap_or(0);
        let src = if is_source(&world.get_block(pos.0, pos.1, pos.2)) { 15 } else { 0 };
        max = max.max(p).max(src);
    }
    max
}

/// Whether any of a block's six neighbours carries redstone power.
fn powered_at(
    power: &std::collections::HashMap<(i32, i32, i32), u32>,
    world: &mut cubeplane_world::World,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    neighbors6(x, y, z)
        .into_iter()
        .any(|(nx, ny, nz)| power.get(&(nx, ny, nz)).copied().unwrap_or(0) > 0 || is_source(&world.get_block(nx, ny, nz)))
}

fn is_source(state: &u16) -> bool {
    let name = block::name_of(*state);
    match name {
        "redstone_block" => true,
        "redstone_torch" | "redstone_wall_torch" => block::prop_index(*state, "lit") == Some(0),
        "lever" => block::prop_index(*state, "powered") == Some(0),
        n if n.ends_with("_pressure_plate") || n.ends_with("_button") => {
            block::prop_index(*state, "powered") == Some(0)
        }
        _ => false,
    }
}

fn neighbors6(x: i32, y: i32, z: i32) -> [(i32, i32, i32); 6] {
    [
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y + 1, z),
        (x, y - 1, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ]
}

/// Detect players standing on pressure plates and power/unpower them, running a
/// redstone update around each plate that changed.
pub fn pressure_plate_tick(shared: &Arc<Shared>) {
    use std::collections::HashSet;
    // The plate a player presses is the block at their feet.
    let mut now: HashSet<(u8, i32, i32, i32)> = HashSet::new();
    for p in shared.players() {
        let s = p.state();
        let (x, y, z) = (s.x.floor() as i32, s.y.floor() as i32, s.z.floor() as i32);
        let name = {
            let mut w = shared.dim_world(s.dimension).lock().unwrap();
            block::name_of(w.get_block(x, y, z))
        };
        if name.ends_with("_pressure_plate") {
            now.insert((s.dimension, x, y, z));
        }
    }
    let (pressed, released) = shared.update_pressed_plates(now);
    for (dim, x, y, z) in pressed {
        set_plate_powered(shared, dim, x, y, z, true);
    }
    for (dim, x, y, z) in released {
        set_plate_powered(shared, dim, x, y, z, false);
    }
}

/// Release buttons whose countdown has expired.
pub fn button_tick(shared: &Arc<Shared>) {
    for (dim, x, y, z) in shared.tick_buttons() {
        set_plate_powered(shared, dim, x, y, z, false);
    }
}

/// Apply delayed block changes (repeater/comparator outputs) that are now due,
/// re-running a redstone update around each so the new signal propagates.
pub fn scheduled_tick(shared: &Arc<Shared>) {
    for (dim, x, y, z, target) in shared.tick_scheduled() {
        let applied = {
            let mut w = shared.dim_world(dim).lock().unwrap();
            // Only apply if the component is still there (it may have been mined).
            let cur = w.get_block(x, y, z);
            if is_logic(block::name_of(cur)) {
                w.set_block(x, y, z, target);
                true
            } else {
                false
            }
        };
        if applied {
            shared.broadcast(cb::block_update(x, y, z, target));
            if dim == 0 {
                redstone_update(shared, x, y, z);
            }
        }
    }
}

/// Set the `powered` property of a plate/button and refresh nearby redstone.
fn set_plate_powered(shared: &Arc<Shared>, dim: u8, x: i32, y: i32, z: i32, on: bool) {
    let updated = {
        let mut w = shared.dim_world(dim).lock().unwrap();
        let state = w.get_block(x, y, z);
        let name = block::name_of(state);
        if !(name.ends_with("_pressure_plate") || name.ends_with("_button")) {
            return;
        }
        let want = block::with_prop(state, "powered", if on { 0 } else { 1 });
        if want == state {
            return;
        }
        w.set_block(x, y, z, want);
        want
    };
    shared.broadcast(cb::block_update(x, y, z, updated));
    if dim == 0 {
        redstone_update(shared, x, y, z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubeplane_world::block;

    #[test]
    fn crop_detection_and_level_states() {
        assert!(is_crop("wheat"));
        assert!(!is_crop("stone"));
        // water level property round-trips through with_prop.
        let w = block::state_by_name("water").unwrap();
        let lvl3 = block::with_prop(w, "level", 3);
        assert_eq!(block::prop_index(lvl3, "level"), Some(3));
    }

    #[test]
    fn flammable_classification() {
        assert!(is_flammable("oak_planks"));
        assert!(is_flammable("birch_log"));
        assert!(!is_flammable("stone"));
    }

    #[test]
    fn redstone_classification() {
        // Sources emit power; actuators react to it; both count as redstone.
        assert!(is_redstone_source("lever"));
        assert!(is_redstone_source("redstone_wire"));
        assert!(is_actuator("redstone_lamp"));
        assert!(is_actuator("oak_door"));
        assert!(is_actuator("oak_trapdoor"));
        assert!(is_actuator("oak_fence_gate"));
        assert!(is_redstone("oak_door"));
        assert!(!is_redstone("stone"));
        // Doors carry an "open" property we can toggle.
        let d = block::state_by_name("oak_door").unwrap();
        let opened = block::with_prop(d, "open", 0);
        assert_eq!(block::prop_index(opened, "open"), Some(0));
    }

    #[test]
    fn logic_components_classified_and_oriented() {
        assert!(is_logic("repeater"));
        assert!(is_logic("comparator"));
        assert!(!is_logic("redstone_wire"));
        assert!(is_redstone("repeater"));
        // 4-value facing maps to unit offsets on the X/Z plane.
        assert_eq!(dir4(0), (0, -1)); // north
        assert_eq!(dir4(1), (0, 1)); // south
        assert_eq!(dir4(2), (-1, 0)); // west
        assert_eq!(dir4(3), (1, 0)); // east
        // A repeater carries a togglable "powered" output and a 1–4 "delay".
        let r = block::state_by_name("repeater").unwrap();
        let on = block::with_prop(r, "powered", 0);
        assert_eq!(block::prop_index(on, "powered"), Some(0));
        let d3 = block::with_prop(r, "delay", 3);
        assert_eq!(block::prop_index(d3, "delay"), Some(3));
    }

    #[test]
    fn pistons_classified_and_pushability() {
        assert!(is_piston("piston"));
        assert!(is_piston("sticky_piston"));
        assert!(is_redstone("piston"));
        // 6-value facing maps to unit offsets in 3D.
        assert_eq!(dir6(0), (0, -1, 0)); // down
        assert_eq!(dir6(1), (0, 1, 0)); // up
        assert_eq!(dir6(5), (1, 0, 0)); // east
        // Ordinary blocks push; bedrock/obsidian/block-entities/air do not.
        assert!(is_pushable(block::state_by_name("stone").unwrap()));
        assert!(is_pushable(block::state_by_name("dirt").unwrap()));
        assert!(!is_pushable(block::state_by_name("bedrock").unwrap()));
        assert!(!is_pushable(block::state_by_name("obsidian").unwrap()));
        assert!(!is_pushable(block::state_by_name("chest").unwrap()));
        assert!(!is_pushable(block::state_by_name("air").unwrap()));
        assert!(!is_pushable(block::state_by_name("piston").unwrap()));
    }

    #[test]
    fn plates_and_buttons_are_sources() {
        assert!(is_redstone_source("stone_pressure_plate"));
        assert!(is_redstone_source("oak_button"));
        // A pressed plate reads as a power source; an unpressed one does not.
        let p = block::state_by_name("stone_pressure_plate").unwrap();
        let on = block::with_prop(p, "powered", 0);
        let off = block::with_prop(p, "powered", 1);
        assert!(is_source(&on));
        assert!(!is_source(&off));
        // An unlit redstone torch is a source; lit-state ordering is index 0.
        let torch = block::state_by_name("redstone_torch").unwrap();
        let lit = block::with_prop(torch, "lit", 0);
        assert!(is_source(&lit));
    }
}
