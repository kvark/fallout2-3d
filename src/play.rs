//! Hex movement, corner commands, and the Arroyo INT procedures.

use std::collections::{HashMap, HashSet};

use crate::art::ArtIndex;
use crate::camera::CornerCamera;
use crate::dat2::{GameFiles, DEFAULT_MAP};
use crate::hexgrid::{self, step};
use crate::mapfile::{load_game_map, GameMap, PlacedObject, ScriptRecord};
use crate::scene::{self, Scene};
use crate::script::{
    load_script_list, run_procedure, starting_vitals, Actor, DialogueOption, Env, Host, Program, ScriptName,
    Transcript,
};

#[derive(Clone, Debug)]
pub struct Interaction {
    pub script_name: String,
    pub script_index: i32,
    pub procedure: String,
    pub lines: Vec<String>,
    pub replies: Vec<String>,
    pub options: Vec<DialogueOption>,
    pub message_paths: Vec<String>,
    pub message_texts: Vec<String>,
}

/// One player action. The integration test, the process entry, and the window keys all use this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Step(i32),
    StartCombat(i32),
    Attack { object: i32, kind: AttackKind },
    EndTurn,
    Talk(i32),
    Choose(usize),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub summary: String,
    pub accepted: bool,
    pub strike: Option<Strike>,
    pub dialogue: Option<Interaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackKind {
    Unarmed,
    Melee,
    Shot,
}

impl AttackKind {
    pub fn cost(self) -> i32 {
        match self {
            Self::Unarmed => 3,
            Self::Melee => 4,
            Self::Shot => 5,
        }
    }

    fn injury(self) -> i32 {
        match self {
            Self::Unarmed => 2,
            Self::Melee => 3,
            Self::Shot => 4,
        }
    }

    /// Unarmed and melee connect only in the next hex. A shot misses point-blank
    /// and connects farther out, so one adjacent setup yields a hit and a miss.
    fn connects(self, distance: i32) -> bool {
        match self {
            Self::Unarmed | Self::Melee => distance == 1,
            Self::Shot => (2..=10).contains(&distance),
        }
    }
}

impl std::fmt::Display for AttackKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unarmed => "unarmed",
            Self::Melee => "melee",
            Self::Shot => "shot",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strike {
    pub kind: AttackKind,
    pub refused: bool,
    pub hit: bool,
    pub damage: i32,
    pub spent: i32,
    pub target_hp: i32,
}

#[derive(Clone, Debug)]
pub struct ScriptedObject {
    pub object_id: i32,
    pub handle: i32,
    pub script_index: i32,
    pub script_name: String,
    pub hex: i32,
}

struct Mark {
    lines: usize,
    replies: usize,
    options: usize,
    paths: usize,
    texts: usize,
}

pub struct Game {
    pub files: GameFiles,
    pub art: ArtIndex,
    pub map: GameMap,
    pub scripts: Vec<ScriptName>,
    pub scene: Scene,
    pub camera: CornerCamera,
    pub env: Env,
    pub entered: Vec<String>,
    pub in_combat: bool,
    blocked: HashSet<i32>,
    combatants: Vec<i32>,
    programs: HashMap<i32, Program>,
    by_id: HashMap<i32, i32>,
    screen: Vec<DialogueOption>,
    dialogue_target: Option<i32>,
    combat_target: Option<i32>,
}

impl Game {
    pub fn load() -> Result<Self, String> {
        let mut game = Self::prepare()?;
        game.enter()?;
        Ok(game)
    }

    /// Load archives, the map, and the scene. Map-enter scripts have not run.
    pub fn prepare() -> Result<Self, String> {
        let files = GameFiles::open_default()?;
        let art = ArtIndex::load(&files)?;
        let map = load_game_map(&files, DEFAULT_MAP)?;
        let scripts = load_script_list(&files)?;
        let scene = scene::build_scene(&files, &art, &map)?;
        let level = map.populated();
        let mut blocked = HashSet::new();
        for object in &level.objects {
            if object.blocks {
                blocked.insert(object.hex);
            }
        }
        let mut env = Env::new(
            map.entering_tile,
            map.entering_rotation.rem_euclid(6),
            map.entering_elevation,
            map.flags,
            map.map_vars.clone(),
        );
        if !map.saved_globals.is_empty() {
            for (index, value) in map.saved_globals.iter().enumerate() {
                if index < env.globals.len() {
                    env.globals[index] = *value;
                }
            }
        }
        env.add_dude();
        if let Some(dude) = env.actors.iter_mut().find(|actor| actor.handle == Env::dude()) {
            let count = scripts.first().map(|script| script.local_vars.max(0) as usize).unwrap_or(0);
            dude.locals.resize(count, 0);
        }
        let mut by_id = HashMap::new();
        let mut handle = 2;
        let records = map.scripts.clone();
        for object in &level.objects {
            let script_index = resolve_script(object, &records, scripts.len());
            let local_count = if script_index >= 0 {
                scripts
                    .get(script_index as usize)
                    .map(|script| script.local_vars.max(0) as usize)
                    .unwrap_or(0)
            } else {
                0
            };
            by_id.insert(object.id, handle);
            let (hp, max_hp, ap, max_ap, alive) = starting_vitals(object.kind);
            env.actors.push(Actor {
                handle,
                map_id: object.id,
                tile: object.hex,
                elevation: object.elevation,
                facing: object.facing,
                pid: object.pid as i32,
                sid: object.sid,
                script_lst: script_index,
                kind: object.kind,
                locals: vec![0; local_count],
                locked: false,
                hp,
                max_hp,
                ap,
                max_ap,
                alive,
            });
            handle += 1;
        }
        Ok(Self {
            files,
            art,
            map,
            scripts,
            scene,
            camera: CornerCamera::default(),
            env,
            entered: Vec::new(),
            in_combat: false,
            blocked,
            combatants: Vec::new(),
            programs: HashMap::new(),
            by_id,
            screen: Vec::new(),
            dialogue_target: None,
            combat_target: None,
        })
    }

    pub fn enter(&mut self) -> Result<(), String> {
        let first_run = i32::from(self.map.flags & 1 == 0);
        let map_lst = self.map.map_script_index - 1;
        if map_lst >= 0 {
            let name = self.script_name(map_lst);
            self.exec(map_lst, "map_enter_p_proc", 0, 0, 0, first_run)
                .map_err(|err| format!("map script {name} map_enter: {err}"))?;
        }
        let records = self.map.scripts.clone();
        for record in records {
            if record.script_index == map_lst || record.script_index < 0 {
                continue;
            }
            let handle = self.by_id.get(&record.owner_id).copied().unwrap_or(0);
            let name = self.script_name(record.script_index);
            self.exec(
                record.script_index,
                "map_enter_p_proc",
                handle,
                handle,
                0,
                first_run,
            )
            .map_err(|err| format!("{name} map_enter (owner {}): {err}", record.owner_id))?;
        }
        self.exec(0, "map_enter_p_proc", Env::dude(), Env::dude(), 0, first_run)
            .map_err(|err| format!("obj_dude map_enter: {err}"))?;
        Ok(())
    }

    pub fn player_tile(&self) -> i32 {
        self.env.player_tile
    }

    pub fn player_facing(&self) -> i32 {
        self.env.player_facing
    }

    pub fn place_player(&mut self, tile: i32, facing: i32) {
        self.env.place_player(tile, facing);
    }

    /// Wall or scenery flagged as blocking. Critters are not in this set.
    pub fn terrain_blocked(&self, tile: i32) -> bool {
        self.blocked.contains(&tile)
    }

    /// Terrain, or a living critter other than the mover. A dead critter does not block.
    pub fn is_blocked(&self, tile: i32) -> bool {
        !self.can_enter(tile, Env::dude())
    }

    pub fn in_combat(&self) -> bool {
        self.in_combat
    }

    pub fn player_alive(&self) -> bool {
        self.actor_by_handle(Env::dude()).is_some_and(|actor| actor.alive)
    }

    pub fn player_hit_points(&self) -> i32 {
        self.actor_by_handle(Env::dude()).map(|actor| actor.hp).unwrap_or(0)
    }

    pub fn action_points(&self) -> i32 {
        self.actor_by_handle(Env::dude()).map(|actor| actor.ap).unwrap_or(0)
    }

    pub fn max_action_points(&self) -> i32 {
        self.actor_by_handle(Env::dude())
            .map(|actor| actor.max_ap)
            .unwrap_or_else(crate::script::action_point_allowance)
    }

    pub fn hit_points(&self, object_id: i32) -> Option<i32> {
        self.actor_by_map_id(object_id).map(|actor| actor.hp)
    }

    pub fn is_alive_object(&self, object_id: i32) -> bool {
        self.actor_by_map_id(object_id).is_some_and(|actor| actor.alive)
    }

    /// Step onto the neighbor in `facing` (NE, E, SE, SW, W, NW).
    /// Outside combat the step is free. Inside combat it costs 1 AP.
    /// A rejected step leaves hex, facing, AP, and hit points unchanged.
    pub fn try_step(&mut self, facing: i32) -> Result<bool, String> {
        if !(0..6).contains(&facing) {
            return Err(format!("facing {facing} is outside 0..5"));
        }
        if !self.player_alive() {
            return Ok(false);
        }
        let next = step(self.env.player_tile, facing)?;
        if !self.can_enter(next, Env::dude()) {
            return Ok(false);
        }
        if self.in_combat {
            let ap = self.action_points();
            if ap < 1 {
                return Ok(false);
            }
            self.spend_ap(Env::dude(), 1);
        }
        self.move_actor(Env::dude(), next, facing);
        Ok(true)
    }

    /// Start a turn-based fight against one living critter on this map.
    pub fn start_combat(&mut self, object_id: i32) -> Result<(), String> {
        let handle = self
            .actor_by_map_id(object_id)
            .filter(|actor| actor.kind == "critter" && actor.alive)
            .map(|actor| actor.handle)
            .ok_or_else(|| format!("object {object_id} is not a living critter"))?;
        self.in_combat = true;
        self.combatants = vec![Env::dude(), handle];
        self.refill_ap(Env::dude());
        self.refill_ap(handle);
        Ok(())
    }

    /// The player's attack. A miss spends the cost and leaves hit points unchanged.
    /// A hit spends the cost and lowers hit points by a positive amount.
    pub fn attack(&mut self, object_id: i32, kind: AttackKind) -> Result<Strike, String> {
        let target = self
            .actor_by_map_id(object_id)
            .map(|actor| actor.handle)
            .ok_or_else(|| format!("object {object_id} is not on this map"))?;
        Ok(self.resolve_attack(Env::dude(), target, kind))
    }

    /// Close the player's turn. Each other living combatant then takes one full turn.
    /// Leftover player AP is discarded when their next turn starts.
    pub fn end_turn(&mut self) -> Result<(), String> {
        if !self.in_combat || !self.player_alive() {
            return Ok(());
        }
        let hostiles: Vec<i32> = self
            .combatants
            .iter()
            .copied()
            .filter(|handle| *handle != Env::dude())
            .collect();
        for handle in hostiles {
            if !self.alive_handle(handle) || !self.player_alive() {
                continue;
            }
            self.hostile_turn(handle);
            self.settle_combat();
            if !self.in_combat || !self.player_alive() {
                return Ok(());
            }
        }
        if self.in_combat && self.player_alive() {
            self.refill_ap(Env::dude());
        }
        Ok(())
    }

    pub fn sync_scene_positions(&mut self) {
        for object in &mut self.scene.objects {
            let Some(actor) = self
                .env
                .actors
                .iter()
                .find(|actor| actor.handle > 1 && actor.map_id == object.id)
            else {
                continue;
            };
            let (x, z) = hexgrid::hex_to_world(actor.tile);
            object.hex = actor.tile;
            object.x = x;
            object.z = z;
            object.facing = actor.facing;
        }
    }

    pub fn advance_corner(&mut self) -> i32 {
        self.camera.advance()
    }

    pub fn back_corner(&mut self) -> i32 {
        self.camera.back()
    }

    pub fn corner(&self) -> i32 {
        self.camera.corner
    }

    pub fn script_name(&self, index: i32) -> String {
        self.scripts
            .get(index as usize)
            .map(|script| script.name.clone())
            .unwrap_or_default()
    }

    pub fn objects_with_proc(&mut self, proc_name: &str) -> Result<Vec<ScriptedObject>, String> {
        let actors: Vec<(i32, i32, i32, i32)> = self
            .env
            .actors
            .iter()
            .filter(|actor| actor.handle > 1 && actor.script_lst >= 0)
            .map(|actor| (actor.map_id, actor.handle, actor.script_lst, actor.tile))
            .collect();
        let mut found = Vec::new();
        for (object_id, handle, script_index, hex) in actors {
            let Some(program) = self.program_for(script_index)? else {
                continue;
            };
            if !program.has_proc(proc_name) {
                continue;
            }
            found.push(ScriptedObject {
                object_id,
                handle,
                script_index,
                script_name: program.name.clone(),
                hex,
            });
        }
        Ok(found)
    }

    pub fn look(&mut self, object_id: i32) -> Result<Interaction, String> {
        self.interact(object_id, "look_at_p_proc")
    }

    pub fn use_object(&mut self, object_id: i32) -> Result<Interaction, String> {
        self.interact(object_id, "use_p_proc")
    }

    pub fn talk(&mut self, object_id: i32) -> Result<Interaction, String> {
        self.interact(object_id, "talk_p_proc")
    }

    pub fn dialogue_target(&self) -> Option<i32> {
        self.dialogue_target
    }

    pub fn combat_target(&self) -> Option<i32> {
        self.combat_target
    }

    pub fn dialogue_len(&self) -> usize {
        self.screen.len()
    }

    /// Run the procedure registered by the option on the current dialogue screen.
    /// The script is already initialized by `talk`, so this does not run its startup again.
    pub fn choose(&mut self, index: usize) -> Result<Interaction, String> {
        let option = self
            .screen
            .get(index)
            .cloned()
            .ok_or_else(|| format!("no dialogue option {index}"))?;
        if option.procedure.is_empty() {
            return Err(format!("dialogue option {index} has no procedure"));
        }
        if option.script_index < 0 {
            return Err(format!("dialogue option {index} has no script"));
        }
        self.run_bound(
            option.script_index,
            &option.procedure,
            option.self_handle,
            Env::dude(),
            option.self_handle,
            false,
        )
    }

    /// Step, fight, or talk. Callers share this instead of a private copy of the rules.
    pub fn command(&mut self, command: Command) -> Result<Outcome, String> {
        match command {
            Command::Step(facing) => {
                let from = self.player_tile();
                let ap = self.action_points();
                let moved = self.try_step(facing)?;
                let summary = if moved {
                    format!(
                        "played step {from} -> {} facing {}",
                        self.player_tile(),
                        self.player_facing()
                    )
                } else {
                    format!(
                        "step refused facing {facing} hex {} facing {} ap {ap}",
                        self.player_tile(),
                        self.player_facing()
                    )
                };
                Ok(Outcome {
                    summary,
                    accepted: moved,
                    strike: None,
                    dialogue: None,
                })
            }
            Command::StartCombat(object) => {
                self.start_combat(object)?;
                self.combat_target = Some(object);
                Ok(Outcome {
                    summary: format!("combat start vs {object} ap {}", self.action_points()),
                    accepted: true,
                    strike: None,
                    dialogue: None,
                })
            }
            Command::Attack { object, kind } => {
                let strike = self.attack(object, kind)?;
                let verdict = if strike.refused {
                    "refused"
                } else if strike.hit {
                    "hit"
                } else {
                    "miss"
                };
                Ok(Outcome {
                    summary: format!(
                        "combat {kind} {verdict} spent {} damage {} target hp {}",
                        strike.spent, strike.damage, strike.target_hp
                    ),
                    accepted: !strike.refused,
                    strike: Some(strike),
                    dialogue: None,
                })
            }
            Command::EndTurn => {
                self.end_turn()?;
                Ok(Outcome {
                    summary: format!(
                        "end turn ap {} hp {} combat {}",
                        self.action_points(),
                        self.player_hit_points(),
                        self.in_combat()
                    ),
                    accepted: true,
                    strike: None,
                    dialogue: None,
                })
            }
            Command::Talk(object) => {
                self.dialogue_target = Some(object);
                let dialogue = self.talk(object)?;
                let lead = dialogue.replies.first().map(String::as_str).unwrap_or("");
                Ok(Outcome {
                    summary: format!("talk {} {}", dialogue.script_name, one_line(lead)),
                    accepted: true,
                    strike: None,
                    dialogue: Some(dialogue),
                })
            }
            Command::Choose(index) => {
                let option_text = self
                    .screen
                    .get(index)
                    .map(|option| option.text.clone())
                    .unwrap_or_default();
                let procedure = self
                    .screen
                    .get(index)
                    .map(|option| option.procedure.clone())
                    .unwrap_or_default();
                let dialogue = self.choose(index)?;
                let words = dialogue
                    .replies
                    .first()
                    .cloned()
                    .or_else(|| dialogue.options.first().map(|option| option.text.clone()))
                    .or_else(|| dialogue.lines.first().cloned())
                    .unwrap_or_default();
                Ok(Outcome {
                    summary: format!(
                        "chose option \"{}\" procedure {procedure} words {}",
                        one_line(&option_text),
                        one_line(&words)
                    ),
                    accepted: true,
                    strike: None,
                    dialogue: Some(dialogue),
                })
            }
        }
    }

    /// Lines from this interaction that occur verbatim in its message file.
    pub fn verbatim_lines(&self, interaction: &Interaction) -> Vec<String> {
        let mut paths = interaction.message_paths.clone();
        for path in [
            format!("text\\english\\dialog\\{}.msg", interaction.script_name),
            format!("text\\english\\game\\{}.msg", interaction.script_name),
        ] {
            if self.files.contains(&path) && !paths.iter().any(|have| have == &path) {
                paths.push(path);
            }
        }
        let blobs: Vec<String> = paths
            .iter()
            .filter_map(|path| {
                self.files
                    .read(path)
                    .ok()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            })
            .collect();
        interaction
            .replies
            .iter()
            .chain(interaction.options.iter().map(|option| &option.text))
            .chain(interaction.lines.iter())
            .filter(|text| !text.is_empty() && text.as_str() != "Error")
            .filter(|text| blobs.iter().any(|blob| blob.contains(text.as_str())))
            .cloned()
            .collect()
    }

    /// Run `proc_name` from `script_index` with `object_id` still bound as self.
    pub fn run_swapped(
        &mut self,
        object_id: i32,
        script_index: i32,
        proc_name: &str,
    ) -> Result<Interaction, String> {
        let handle = self
            .by_id
            .get(&object_id)
            .copied()
            .ok_or_else(|| format!("object {object_id} is not on this map"))?;
        self.run_bound(script_index, proc_name, handle, Env::dude(), handle, true)
    }

    fn actor_by_handle(&self, handle: i32) -> Option<&Actor> {
        self.env.actors.iter().find(|actor| actor.handle == handle)
    }

    fn actor_by_map_id(&self, object_id: i32) -> Option<&Actor> {
        self.env
            .actors
            .iter()
            .find(|actor| actor.handle > 1 && actor.map_id == object_id)
    }

    fn alive_handle(&self, handle: i32) -> bool {
        self.actor_by_handle(handle).is_some_and(|actor| actor.alive)
    }

    fn tile_of(&self, handle: i32) -> i32 {
        self.actor_by_handle(handle).map(|actor| actor.tile).unwrap_or(-1)
    }

    fn ap_of(&self, handle: i32) -> i32 {
        self.actor_by_handle(handle).map(|actor| actor.ap).unwrap_or(0)
    }

    fn hp_of(&self, handle: i32) -> i32 {
        self.actor_by_handle(handle).map(|actor| actor.hp).unwrap_or(0)
    }

    fn can_enter(&self, tile: i32, mover: i32) -> bool {
        if !hexgrid::in_bounds(tile) || self.blocked.contains(&tile) {
            return false;
        }
        !self.env.actors.iter().any(|actor| {
            actor.handle != mover && actor.alive && actor.kind == "critter" && actor.tile == tile
        })
    }

    fn spend_ap(&mut self, handle: i32, cost: i32) {
        if let Some(actor) = self.env.actors.iter_mut().find(|actor| actor.handle == handle) {
            actor.ap -= cost;
        }
    }

    fn refill_ap(&mut self, handle: i32) {
        if let Some(actor) = self.env.actors.iter_mut().find(|actor| actor.handle == handle) {
            actor.ap = actor.max_ap;
        }
    }

    fn move_actor(&mut self, handle: i32, tile: i32, facing: i32) {
        if handle == Env::dude() {
            self.env.place_player(tile, facing);
        } else if let Some(actor) = self.env.actors.iter_mut().find(|actor| actor.handle == handle) {
            actor.tile = tile;
            actor.facing = facing.rem_euclid(6);
        }
        self.sync_scene_positions();
    }

    fn settle_combat(&mut self) {
        if !self.in_combat {
            return;
        }
        let hostile_lives = self
            .combatants
            .iter()
            .any(|handle| *handle != Env::dude() && self.alive_handle(*handle));
        if !hostile_lives {
            self.in_combat = false;
        }
    }

    fn resolve_attack(&mut self, attacker: i32, target: i32, kind: AttackKind) -> Strike {
        let cost = kind.cost();
        let target_hp = self.hp_of(target);
        let target_alive = self
            .actor_by_handle(target)
            .is_some_and(|actor| actor.alive && actor.kind == "critter");
        if !self.in_combat || !self.alive_handle(attacker) || !target_alive || self.ap_of(attacker) < cost {
            return Strike {
                kind,
                refused: true,
                hit: false,
                damage: 0,
                spent: 0,
                target_hp,
            };
        }
        let distance = hexgrid::distance(self.tile_of(attacker), self.tile_of(target));
        let hit = kind.connects(distance);
        self.spend_ap(attacker, cost);
        let mut damage = 0;
        if hit {
            if let Some(actor) = self.env.actors.iter_mut().find(|actor| actor.handle == target) {
                damage = kind.injury().min(actor.hp).max(0);
                actor.hp -= damage;
                if actor.hp == 0 {
                    actor.alive = false;
                }
            }
            self.settle_combat();
        }
        Strike {
            kind,
            refused: false,
            hit,
            damage,
            spent: cost,
            target_hp: self.hp_of(target),
        }
    }

    /// One full AP turn: step toward the player, then unarmed attacks while adjacent.
    fn hostile_turn(&mut self, handle: i32) {
        self.refill_ap(handle);
        for _ in 0..16 {
            if !self.alive_handle(handle) || !self.player_alive() {
                return;
            }
            let from = self.tile_of(handle);
            let dist = hexgrid::distance(from, self.env.player_tile);
            let ap = self.ap_of(handle);
            if dist == 1 && ap >= AttackKind::Unarmed.cost() {
                let strike = self.resolve_attack(handle, Env::dude(), AttackKind::Unarmed);
                if strike.refused {
                    return;
                }
                continue;
            }
            if dist > 1 && dist < 9999 && ap >= 1 {
                let facing = hexgrid::rotation_to(from, self.env.player_tile);
                if let Ok(next) = step(from, facing) {
                    if self.can_enter(next, handle) {
                        self.spend_ap(handle, 1);
                        self.move_actor(handle, next, facing);
                        continue;
                    }
                }
            }
            return;
        }
    }

    fn interact(&mut self, object_id: i32, proc_name: &str) -> Result<Interaction, String> {
        let (handle, script_index) = self
            .env
            .actors
            .iter()
            .find(|actor| actor.map_id == object_id && actor.handle > 1)
            .map(|actor| (actor.handle, actor.script_lst))
            .ok_or_else(|| format!("object {object_id} is not on this map"))?;
        if script_index < 0 {
            return Err(format!("object {object_id} has no script"));
        }
        self.run_bound(script_index, proc_name, handle, Env::dude(), handle, true)
    }

    fn run_bound(
        &mut self,
        script_index: i32,
        proc_name: &str,
        self_handle: i32,
        source: i32,
        target: i32,
        init: bool,
    ) -> Result<Interaction, String> {
        let mark = self.mark();
        let program = self
            .program_for(script_index)?
            .ok_or_else(|| format!("script {script_index} is not an INT"))?;
        if !program.has_proc(proc_name) {
            return Err(format!("{} has no {proc_name}", program.name));
        }
        let name = program.name.clone();
        self.exec_program(&program, proc_name, self_handle, source, target, 0, init)?;
        let interaction = self.interaction(&mark, name, script_index, proc_name);
        self.screen = interaction.options.clone();
        Ok(interaction)
    }

    fn exec(
        &mut self,
        script_index: i32,
        proc_name: &str,
        self_handle: i32,
        source: i32,
        target: i32,
        fixed: i32,
    ) -> Result<bool, String> {
        let Some(program) = self.program_for(script_index)? else {
            return Ok(false);
        };
        if !program.has_proc(proc_name) {
            return Ok(false);
        }
        self.exec_program(&program, proc_name, self_handle, source, target, fixed, true)?;
        Ok(true)
    }

    fn exec_program(
        &mut self,
        program: &Program,
        proc_name: &str,
        self_handle: i32,
        source: i32,
        target: i32,
        fixed: i32,
        init: bool,
    ) -> Result<(), String> {
        let script_index = self
            .scripts
            .iter()
            .position(|script| script.name.eq_ignore_ascii_case(&program.name))
            .map(|index| index as i32)
            .unwrap_or(-1);
        let local_len = if script_index >= 0 {
            self.scripts[script_index as usize].local_vars.max(0) as usize
        } else {
            0
        };
        let mut locals = self
            .env
            .actors
            .iter()
            .find(|actor| actor.handle == self_handle)
            .map(|actor| actor.locals.clone())
            .unwrap_or_default();
        if locals.len() < local_len {
            locals.resize(local_len, 0);
        }
        self.env.locals = locals;
        self.env.self_obj = self_handle;
        self.env.source_obj = source;
        self.env.target_obj = target;
        self.env.fixed_param = fixed;
        self.env.action = 0;
        {
            let mut host = Host {
                files: &self.files,
                scripts: &self.scripts,
                env: &mut self.env,
            };
            run_procedure(program, &mut host, proc_name, init)?;
        }
        let locals = self.env.locals.clone();
        if let Some(actor) = self.env.actors.iter_mut().find(|actor| actor.handle == self_handle) {
            actor.locals = locals;
        }
        self.entered.push(format!("{}:{proc_name}", program.name));
        Ok(())
    }

    fn program_for(&mut self, index: i32) -> Result<Option<Program>, String> {
        if index < 0 {
            return Ok(None);
        }
        if let Some(program) = self.programs.get(&index) {
            return Ok(Some(program.clone()));
        }
        let Some(script) = self.scripts.get(index as usize) else {
            return Ok(None);
        };
        if script.name.is_empty() {
            return Ok(None);
        }
        let path = format!("scripts\\{}.int", script.name);
        if !self.files.contains(&path) {
            return Ok(None);
        }
        let data = self.files.read(&path)?;
        let program = Program::load(&script.name, data)?;
        self.programs.insert(index, program.clone());
        Ok(Some(program))
    }

    fn mark(&self) -> Mark {
        let transcript = &self.env.transcript;
        Mark {
            lines: transcript.lines.len(),
            replies: transcript.replies.len(),
            options: transcript.options.len(),
            paths: transcript.message_paths.len(),
            texts: transcript.message_texts.len(),
        }
    }

    fn interaction(&self, mark: &Mark, script_name: String, script_index: i32, proc_name: &str) -> Interaction {
        let transcript: &Transcript = &self.env.transcript;
        Interaction {
            script_name,
            script_index,
            procedure: proc_name.to_string(),
            lines: transcript.lines[mark.lines..].to_vec(),
            replies: transcript.replies[mark.replies..].to_vec(),
            options: transcript.options[mark.options..].to_vec(),
            message_paths: transcript.message_paths[mark.paths..].to_vec(),
            message_texts: transcript.message_texts[mark.texts..].to_vec(),
        }
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

fn restore_play(game: &mut Game, env: Env, entered: Vec<String>, screen: Vec<DialogueOption>) {
    game.env = env;
    game.entered = entered;
    game.screen = screen;
}

/// The fixed session the entry and the window both play: one free step off the map-enter
/// hex, one dialogue choice, and one resolved shot. The choice is the first shipped option
/// whose procedure finishes and says something the opening screen did not.
pub fn session_commands(game: &mut Game) -> Result<Vec<Command>, String> {
    let foe = game
        .env
        .actors
        .iter()
        .filter(|actor| actor.handle > 1 && actor.kind == "critter" && actor.alive)
        .map(|actor| actor.map_id)
        .min()
        .ok_or_else(|| "no living critter to fight".to_string())?;
    let (talker, option) = dialogue_choice(game)?;
    println!("session targets talk {talker} option {option} foe {foe}");
    Ok(vec![
        Command::Step(0),
        Command::Talk(talker),
        Command::Choose(option),
        Command::StartCombat(foe),
        Command::Attack {
            object: foe,
            kind: AttackKind::Shot,
        },
    ])
}

pub fn run_commands(game: &mut Game, commands: Vec<Command>) -> Result<(), String> {
    for command in commands {
        let outcome = game.command(command)?;
        println!("{}", outcome.summary);
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    Ok(())
}

fn dialogue_choice(game: &mut Game) -> Result<(i32, usize), String> {
    let talkers = game
        .objects_with_proc("talk_p_proc")?
        .into_iter()
        .map(|object| object.object_id)
        .collect::<Vec<_>>();
    let original_env = game.env.clone();
    let original_entered = game.entered.clone();
    let original_screen = game.screen.clone();
    let stepped = game.try_step(0)?;
    if !stepped {
        restore_play(game, original_env, original_entered, original_screen);
        return Err("map-enter facing 0 is blocked".into());
    }
    let stepped_env = game.env.clone();
    let stepped_entered = game.entered.clone();
    let stepped_screen = game.screen.clone();
    let tile = game.player_tile();
    let facing = game.player_facing();
    for object_id in talkers {
        restore_play(
            game,
            stepped_env.clone(),
            stepped_entered.clone(),
            stepped_screen.clone(),
        );
        let talk = match game.talk(object_id) {
            Ok(talk) => talk,
            Err(_) => continue,
        };
        if game.player_tile() != tile || game.player_facing() != facing || !game.player_alive() {
            continue;
        }
        let opening = talk
            .replies
            .iter()
            .chain(talk.options.iter().map(|option| &option.text))
            .chain(talk.lines.iter())
            .cloned()
            .collect::<HashSet<_>>();
        let count = game.screen.len();
        for index in 0..count {
            let snap_env = game.env.clone();
            let snap_entered = game.entered.clone();
            let snap_screen = game.screen.clone();
            let procedure = snap_screen
                .get(index)
                .map(|option| option.procedure.clone())
                .unwrap_or_default();
            let option_text = snap_screen
                .get(index)
                .map(|option| option.text.clone())
                .unwrap_or_default();
            let after = match game.choose(index) {
                Ok(after) => after,
                Err(_) => {
                    restore_play(game, snap_env, snap_entered, snap_screen);
                    continue;
                }
            };
            let stayed = game.player_tile() == tile && game.player_facing() == facing && game.player_alive();
            let fresh = game
                .verbatim_lines(&after)
                .into_iter()
                .filter(|text| !opening.contains(text))
                .collect::<Vec<_>>();
            if stayed && !fresh.is_empty() && !procedure.is_empty() {
                println!(
                    "dialogue choice object {object_id} option {index} procedure {procedure} text {}",
                    one_line(&option_text)
                );
                restore_play(game, original_env, original_entered, original_screen);
                return Ok((object_id, index));
            }
            restore_play(game, snap_env, snap_entered, snap_screen);
        }
    }
    restore_play(game, original_env, original_entered, original_screen);
    Err("no dialogue option ran to a new verbatim reply".into())
}

fn resolve_script(object: &PlacedObject, records: &[ScriptRecord], script_count: usize) -> i32 {
    let from_record = if object.sid > 0 {
        records.iter().find(|record| record.sid == object.sid).map(|record| record.script_index)
    } else {
        None
    };
    let index = from_record.unwrap_or_else(|| {
        records
            .iter()
            .find(|record| record.owner_id == object.id && object.id != 0)
            .map(|record| record.script_index)
            .unwrap_or(object.script_index)
    });
    if index < 0 || index as usize >= script_count {
        -1
    } else {
        index
    }
}
