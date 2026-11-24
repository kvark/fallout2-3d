//! INT bytecode interpreter. Opcodes follow the Fallout 2 engine's stack effects.
//! An opcode the running procedure reaches and this VM does not implement is an error.

use std::collections::HashMap;

use crate::dat2::GameFiles;
use crate::hexgrid::{self, HEX_COUNT};

const PROC_TABLE: usize = 42;
const RETURN_EXIT: i32 = 24;
const MAX_STEPS: u32 = 200_000;

const F_EXITED: u32 = 0x01;
const F_FATAL: u32 = 0x04;
const F_STOPPED: u32 = 0x08;
const F_FINISHED: u32 = 0x40;
const F_CRITICAL: u32 = 0x80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Float,
    Str,
    Dyn,
    Ptr,
}

#[derive(Clone, Copy, Debug)]
struct Val {
    kind: Kind,
    bits: i32,
    float: f32,
}

impl Val {
    fn int(bits: i32) -> Self {
        Self { kind: Kind::Int, bits, float: 0.0 }
    }
    fn ptr(bits: i32) -> Self {
        Self { kind: Kind::Ptr, bits, float: 0.0 }
    }
    fn stat(bits: i32) -> Self {
        Self { kind: Kind::Str, bits, float: 0.0 }
    }
    fn r#dyn(bits: i32) -> Self {
        Self { kind: Kind::Dyn, bits, float: 0.0 }
    }
    fn from_push(opcode: u16, raw: i32) -> Self {
        let op = opcode as u32;
        if op & 0x0800 != 0 {
            return Self::r#dyn(raw);
        }
        if op & 0x1000 != 0 {
            return Self::stat(raw);
        }
        if op & 0x6000 == 0x6000 {
            return Self::ptr(raw);
        }
        if op & 0x2000 != 0 {
            return Self {
                kind: Kind::Float,
                bits: raw,
                float: f32::from_bits(raw as u32),
            };
        }
        Self::int(raw)
    }
    fn empty(self) -> bool {
        match self.kind {
            Kind::Float => self.float == 0.0,
            _ => self.bits == 0,
        }
    }
    fn as_int(self) -> i32 {
        match self.kind {
            Kind::Float => self.float as i32,
            _ => self.bits,
        }
    }
}

#[derive(Clone, Debug)]
struct Proc {
    name: String,
    flags: i32,
    body: i32,
    argc: i32,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub name: String,
    data: Vec<u8>,
    procs: Vec<Proc>,
    ident_base: usize,
    strings_base: usize,
}

impl Program {
    pub fn load(name: &str, data: Vec<u8>) -> Result<Self, String> {
        if data.len() < PROC_TABLE + 4 {
            return Err(format!("{name} is too small to be an INT"));
        }
        let count = be_i32(&data, PROC_TABLE) as usize;
        let procs_at = PROC_TABLE + 4;
        if procs_at + count * 24 > data.len() {
            return Err(format!("{name} procedure table overruns the file"));
        }
        let ident_base = procs_at + count * 24;
        let mut procs = Vec::with_capacity(count);
        for index in 0..count {
            let at = procs_at + index * 24;
            let name_off = be_i32(&data, at);
            let flags = be_i32(&data, at + 4);
            let body = be_i32(&data, at + 16);
            let argc = be_i32(&data, at + 20);
            let proc_name = cstr_at(&data, ident_base.saturating_add(name_off as usize));
            procs.push(Proc {
                name: proc_name,
                flags,
                body,
                argc,
            });
        }
        let ident_size = if ident_base + 4 <= data.len() {
            be_i32(&data, ident_base).max(0) as usize
        } else {
            0
        };
        let strings_base = ident_base + ident_size + 4;
        Ok(Self {
            name: name.to_string(),
            data,
            procs,
            ident_base,
            strings_base,
        })
    }

    pub fn has_proc(&self, name: &str) -> bool {
        self.proc_index(name).is_some()
    }

    fn proc_index(&self, name: &str) -> Option<usize> {
        self.procs.iter().position(|proc| proc.name.eq_ignore_ascii_case(name))
    }

    fn static_str(&self, offset: i32) -> String {
        cstr_at(&self.data, self.strings_base.saturating_add(4).saturating_add(offset as usize))
    }

    fn ident(&self, offset: i32) -> String {
        cstr_at(&self.data, self.ident_base.saturating_add(offset as usize))
    }
}

#[derive(Clone, Debug)]
pub struct ScriptName {
    pub name: String,
    pub local_vars: i32,
}

pub fn load_script_list(files: &GameFiles) -> Result<Vec<ScriptName>, String> {
    let text = String::from_utf8_lossy(&files.read("scripts\\scripts.lst")?).into_owned();
    let mut entries = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() && entries.is_empty() {
            // Keep blank lines that are real list entries. A trailing split empty is dropped below.
        }
        let name = line
            .split(".int")
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(13)
            .collect::<String>();
        let local_vars = line
            .find("local_vars=")
            .and_then(|at| line[at + "local_vars=".len()..].split(|ch: char| !ch.is_ascii_digit() && ch != '-').next())
            .and_then(|token| token.parse::<i32>().ok())
            .unwrap_or(0);
        entries.push(ScriptName { name, local_vars });
    }
    if text.ends_with('\n') {
        if entries.last().is_some_and(|entry| entry.name.is_empty()) {
            entries.pop();
        }
    }
    Ok(entries)
}

#[derive(Clone, Debug)]
pub struct Actor {
    pub handle: i32,
    pub map_id: i32,
    pub tile: i32,
    pub elevation: i32,
    pub facing: i32,
    pub pid: i32,
    pub sid: i32,
    pub script_lst: i32,
    pub kind: &'static str,
    pub locals: Vec<i32>,
    pub locked: bool,
    pub hp: i32,
    pub max_hp: i32,
    pub ap: i32,
    pub max_ap: i32,
    pub alive: bool,
}

/// SPECIAL and the few derived stats these Arroyo procedures read.
pub fn stat_value(stat: i32) -> i32 {
    match stat {
        0..=6 => 5,
        34 => 0,
        _ => 1,
    }
}

/// Classic action-point allowance: 5 + Agility / 2.
pub fn action_point_allowance() -> i32 {
    5 + stat_value(5) / 2
}

/// Unread proto combat fields use a fixed pool. The player uses the SPECIAL formula.
pub fn starting_vitals(kind: &str) -> (i32, i32, i32, i32, bool) {
    if kind == "critter" {
        let max_ap = action_point_allowance();
        (8, 8, max_ap, max_ap, true)
    } else {
        (1, 1, 0, 0, true)
    }
}

/// One line offered by `gsay_option` / `giq_option`, including the procedure it runs.
#[derive(Clone, Debug)]
pub struct DialogueOption {
    pub text: String,
    pub procedure: String,
    pub script_index: i32,
    pub self_handle: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub lines: Vec<String>,
    pub replies: Vec<String>,
    pub options: Vec<DialogueOption>,
    pub procedures: Vec<String>,
    pub message_paths: Vec<String>,
    pub message_texts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Env {
    pub actors: Vec<Actor>,
    pub player_tile: i32,
    pub player_facing: i32,
    pub player_elevation: i32,
    pub globals: Vec<i32>,
    pub map_vars: Vec<i32>,
    pub map_flags: i32,
    pub hour: i32,
    pub month: i32,
    pub ticks: i32,
    pub rng: u32,
    pub self_obj: i32,
    pub source_obj: i32,
    pub target_obj: i32,
    pub action: i32,
    pub fixed_param: i32,
    pub overrides: i32,
    pub locals: Vec<i32>,
    pub xp: i32,
    pub light: i32,
    pub transcript: Transcript,
    msg_cache: HashMap<i32, HashMap<i32, String>>,
    msg_path: HashMap<i32, String>,
    extern_vars: HashMap<String, Val>,
    /// Game opcodes reached through a fallback, by dispatch index.
    pub stub_ops: Vec<u16>,
}

impl Env {
    pub fn new(player_tile: i32, player_facing: i32, player_elevation: i32, map_flags: i32, map_vars: Vec<i32>) -> Self {
        Self {
            actors: vec![Actor {
                handle: 0,
                map_id: 0,
                tile: -1,
                elevation: 0,
                facing: 0,
                pid: 0,
                sid: -1,
                script_lst: -1,
                kind: "null",
                locals: Vec::new(),
                locked: false,
                hp: 0,
                max_hp: 0,
                ap: 0,
                max_ap: 0,
                alive: false,
            }],
            player_tile,
            player_facing,
            player_elevation,
            globals: vec![0; 2048],
            map_vars,
            map_flags,
            hour: 8,
            month: 1,
            ticks: 8 * 36000,
            rng: 1,
            self_obj: 0,
            source_obj: 0,
            target_obj: 0,
            action: 0,
            fixed_param: 0,
            overrides: 0,
            locals: Vec::new(),
            xp: 0,
            light: 50,
            transcript: Transcript::default(),
            msg_cache: HashMap::new(),
            msg_path: HashMap::new(),
            extern_vars: HashMap::new(),
            stub_ops: Vec::new(),
        }
    }

    pub fn note_stub(&mut self, index: u16) {
        if !self.stub_ops.contains(&index) {
            self.stub_ops.push(index);
        }
    }

    pub fn dude() -> i32 {
        1
    }

    pub fn add_dude(&mut self) {
        let max_hp = 15 + stat_value(0) + 2 * stat_value(2);
        let max_ap = action_point_allowance();
        self.actors.push(Actor {
            handle: 1,
            map_id: 1,
            tile: self.player_tile,
            elevation: self.player_elevation,
            facing: self.player_facing,
            pid: 0x0100_0000,
            sid: -1,
            script_lst: 0,
            kind: "critter",
            locals: Vec::new(),
            locked: false,
            hp: max_hp,
            max_hp,
            ap: max_ap,
            max_ap,
            alive: true,
        });
    }

    pub fn place_player(&mut self, tile: i32, facing: i32) {
        let facing = facing.rem_euclid(6);
        self.player_tile = tile;
        self.player_facing = facing;
        if let Some(dude) = self.actors.iter_mut().find(|actor| actor.handle == Self::dude()) {
            dude.tile = tile;
            dude.facing = facing;
        }
    }

    fn actor_mut(&mut self, handle: i32) -> Option<&mut Actor> {
        self.actors.iter_mut().find(|actor| actor.handle == handle)
    }

    fn actor(&self, handle: i32) -> Option<&Actor> {
        self.actors.iter().find(|actor| actor.handle == handle)
    }

    pub fn base_stat(&self, stat: i32) -> i32 {
        stat_value(stat)
    }

    fn stat(&self, stat: i32) -> i32 {
        stat_value(stat)
    }

    fn random(&mut self, min: i32, max: i32) -> i32 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        let span = (hi as i64 - lo as i64 + 1).max(1) as u32;
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        lo.saturating_add((self.rng % span) as i32)
    }

    fn ensure_global(&mut self, index: i32) -> Result<(), String> {
        if index < 0 {
            return Err(format!("global var {index} is negative"));
        }
        let need = index as usize + 1;
        if self.globals.len() < need {
            self.globals.resize(need, 0);
        }
        Ok(())
    }
}

struct Machine {
    program: Program,
    ip: i32,
    frame: i32,
    base: i32,
    flags: u32,
    stack: Vec<Val>,
    ret: Vec<Val>,
    dynamic: Vec<String>,
}

impl Machine {
    fn new(program: Program) -> Self {
        Self {
            program,
            ip: 0,
            frame: -1,
            base: -1,
            flags: 0,
            stack: Vec::new(),
            ret: Vec::new(),
            dynamic: vec![String::new()],
        }
    }

    fn push(&mut self, value: Val) {
        self.stack.push(value);
    }

    fn pop(&mut self) -> Result<Val, String> {
        self.stack.pop().ok_or_else(|| {
            format!(
                "{} stack underflow at ip {:#x}",
                self.program.name, self.ip
            )
        })
    }

    fn pop_int(&mut self) -> Result<i32, String> {
        Ok(self.pop()?.as_int())
    }

    fn rpush(&mut self, value: Val) {
        self.ret.push(value);
    }

    fn rpop(&mut self) -> Result<Val, String> {
        self.ret.pop().ok_or_else(|| {
            format!("{} return stack underflow at ip {:#x}", self.program.name, self.ip)
        })
    }

    fn text(&self, value: Val) -> String {
        match value.kind {
            Kind::Str => self.program.static_str(value.bits),
            Kind::Dyn => self.dynamic.get(value.bits as usize).cloned().unwrap_or_default(),
            Kind::Float => format!("{}", value.float),
            _ => value.bits.to_string(),
        }
    }

    fn intern(&mut self, text: String) -> Val {
        if let Some(index) = self.dynamic.iter().position(|have| have == &text) {
            return Val::r#dyn(index as i32);
        }
        let index = self.dynamic.len();
        self.dynamic.push(text);
        Val::r#dyn(index as i32)
    }

    fn read_op(&mut self) -> Result<u16, String> {
        let at = self.ip as usize;
        if at + 2 > self.program.data.len() {
            return Err(format!("{} opcode ran off the end at {at}", self.program.name));
        }
        let opcode = u16::from_be_bytes([self.program.data[at], self.program.data[at + 1]]);
        self.ip += 2;
        if opcode & 0x8000 == 0 {
            return Err(format!("{} bad opcode {opcode:#x} at {at}", self.program.name));
        }
        Ok(opcode)
    }

    fn read_i32(&mut self) -> Result<i32, String> {
        let at = self.ip as usize;
        if at + 4 > self.program.data.len() {
            return Err(format!("{} immediate ran off the end at {at}", self.program.name));
        }
        let value = be_i32(&self.program.data, at);
        self.ip += 4;
        Ok(value)
    }

    fn setup_call(&mut self, address: i32, return_address: i32) {
        self.rpush(Val::int(self.ip));
        self.rpush(Val::int(return_address));
        self.push(Val::int((self.flags & 0xFFFF) as i32));
        self.push(Val::ptr(0));
        self.push(Val::int(0));
        self.flags &= !0xFFFF;
        self.ip = address;
        self.push(Val::int(0));
    }

    fn interpret(&mut self, host: &mut Host, unlimited: bool) -> Result<(), String> {
        let mut steps = 0u32;
        while self.flags & (F_EXITED | F_FATAL | F_STOPPED | F_FINISHED) == 0 {
            steps += 1;
            if steps > MAX_STEPS {
                return Err(format!(
                    "{} exceeded {MAX_STEPS} instructions at ip {:#x}",
                    self.program.name, self.ip
                ));
            }
            if !unlimited && self.flags & F_CRITICAL == 0 && steps > 24 {
                break;
            }
            let opcode = self.read_op()?;
            let index = opcode & 0x3FF;
            if index == 1 {
                let raw = self.read_i32()?;
                self.push(Val::from_push(opcode, raw));
                continue;
            }
            self.exec(index, host)?;
        }
        self.flags &= !F_FINISHED;
        Ok(())
    }

    fn exec(&mut self, index: u16, host: &mut Host) -> Result<(), String> {
        if index >= 0x0A1 {
            return self.game(index, host);
        }
        match index {
            0x000 => Ok(()),
            0x002 => {
                self.flags |= F_CRITICAL;
                Ok(())
            }
            0x003 | 0x04B => {
                self.flags &= !F_CRITICAL;
                Ok(())
            }
            0x04A => {
                self.flags |= F_CRITICAL;
                Ok(())
            }
            0x004 => {
                self.ip = self.pop_int()?;
                Ok(())
            }
            0x005 => self.op_call(),
            0x006 | 0x007 => {
                let _a = self.pop_int()?;
                let _b = self.pop_int()?;
                Ok(())
            }
            0x008 | 0x00A | 0x00B => {
                let value = self.pop()?;
                let name = self.text(value);
                Err(format!("{} callstart/spawn/fork of {name} is not used by the Arroyo play loop", self.program.name))
            }
            0x009 => Err(format!("{} exec is not used by the Arroyo play loop", self.program.name)),
            0x00C => {
                let value = self.rpop()?;
                self.push(value);
                Ok(())
            }
            0x00D => {
                let value = self.pop()?;
                self.rpush(value);
                Ok(())
            }
            0x00E | 0x010 => {
                self.flags |= F_EXITED;
                Ok(())
            }
            0x011 => {
                self.flags |= F_STOPPED;
                Ok(())
            }
            0x00F => Ok(()),
            0x012 => self.op_fetch_global(),
            0x013 => self.op_store_global(),
            0x014 => self.op_fetch_external(host),
            0x015 => self.op_store_external(host),
            0x016 => self.op_export_var(host),
            0x017 => self.op_export_proc(host),
            0x018 => {
                let top = self.pop()?;
                let next = self.pop()?;
                self.push(top);
                self.push(next);
                Ok(())
            }
            0x019 => {
                let top = self.rpop()?;
                let next = self.rpop()?;
                self.rpush(top);
                self.rpush(next);
                Ok(())
            }
            0x01A => {
                self.pop()?;
                Ok(())
            }
            0x01B => {
                let value = self.pop()?;
                self.push(value);
                self.push(value);
                Ok(())
            }
            0x01C => {
                self.ip = self.rpop()?.as_int();
                Ok(())
            }
            0x01D => {
                self.ip = self.rpop()?.as_int();
                self.flags |= F_FINISHED;
                Ok(())
            }
            0x01E => {
                self.rpop()?;
                Ok(())
            }
            0x01F => self.op_pop_flags(),
            0x020 => {
                self.op_pop_flags()?;
                self.ip = self.rpop()?.as_int();
                Ok(())
            }
            0x021 => {
                self.op_pop_flags()?;
                self.ip = self.rpop()?.as_int();
                self.flags |= F_FINISHED;
                Ok(())
            }
            0x022 | 0x023 | 0x024 | 0x026 => Err(format!(
                "{} external return {index:#x} is not used by these Arroyo procedures",
                self.program.name
            )),
            0x025 => {
                let value = self.pop()?;
                self.op_pop_flags()?;
                self.ip = self.rpop()?.as_int();
                self.flags |= F_FINISHED;
                self.push(value);
                Ok(())
            }
            0x027 => self.op_check_argc(),
            0x028 => self.op_lookup(),
            0x029 => {
                self.frame = self.rpop()?.as_int();
                Ok(())
            }
            0x02A => {
                while self.stack.len() != self.frame.max(0) as usize {
                    if self.stack.len() < self.frame.max(0) as usize {
                        break;
                    }
                    if self.stack.len() == self.frame as usize {
                        break;
                    }
                    self.pop()?;
                }
                Ok(())
            }
            0x02B => {
                let argc = self.pop_int()?;
                self.rpush(Val::int(self.frame));
                self.frame = self.stack.len() as i32 - argc;
                Ok(())
            }
            0x02C => {
                self.base = self.stack.len() as i32;
                Ok(())
            }
            0x02D => {
                let index = self.pop_int()? as usize;
                let proc = self.program.procs.get(index).ok_or_else(|| {
                    format!("{} fetch proc {index} is out of range", self.program.name)
                })?;
                self.push(Val::int(proc.body));
                Ok(())
            }
            0x02E => {
                let count = self.pop_int()?;
                for _ in 0..count {
                    self.pop()?;
                }
                Ok(())
            }
            0x02F => {
                let value = self.pop()?;
                if !value.empty() {
                    self.pop()?;
                } else {
                    self.ip = self.pop_int()?;
                }
                Ok(())
            }
            0x030 => {
                let value = self.pop()?;
                if value.empty() {
                    self.ip = self.pop_int()?;
                }
                Ok(())
            }
            0x031 => self.op_store(false),
            0x032 => self.op_fetch(false),
            0x033 => self.op_cmp(host, Cmp::Eq),
            0x034 => self.op_cmp(host, Cmp::Ne),
            0x035 => self.op_cmp(host, Cmp::Le),
            0x036 => self.op_cmp(host, Cmp::Ge),
            0x037 => self.op_cmp(host, Cmp::Lt),
            0x038 => self.op_cmp(host, Cmp::Gt),
            0x039 => self.op_arith(Arith::Add),
            0x03A => self.op_arith(Arith::Sub),
            0x03B => self.op_arith(Arith::Mul),
            0x03C => self.op_arith(Arith::Div),
            0x03D => self.op_arith(Arith::Mod),
            0x03E => self.op_logic(false),
            0x03F => self.op_logic(true),
            0x040 => self.op_bit(Bit::And),
            0x041 => self.op_bit(Bit::Or),
            0x042 => self.op_bit(Bit::Xor),
            0x043 => {
                let value = self.pop_int()?;
                self.push(Val::int(!value));
                Ok(())
            }
            0x044 => {
                let value = self.pop()?;
                let number = if value.kind == Kind::Float { value.float } else { value.bits as f32 };
                self.push(Val::int(number.floor() as i32));
                Ok(())
            }
            0x045 => {
                let value = self.pop()?;
                self.push(Val::int(if value.empty() { 1 } else { 0 }));
                Ok(())
            }
            0x046 => {
                let value = self.pop()?;
                if value.kind == Kind::Float {
                    self.push(Val {
                        kind: Kind::Float,
                        bits: 0,
                        float: -value.float,
                    });
                } else {
                    self.push(Val::int(-value.as_int()));
                }
                Ok(())
            }
            0x047 => {
                let _ticks = self.pop_int()?;
                Ok(())
            }
            0x048 => {
                let _proc = self.pop_int()?;
                Ok(())
            }
            0x049 => Ok(()),
            other => Err(format!(
                "{} unimplemented core opcode {:#x} at ip {:#x}",
                self.program.name,
                0x8000 | other,
                self.ip
            )),
        }
    }

    fn op_call(&mut self) -> Result<(), String> {
        let index = self.pop_int()? as usize;
        let proc = self.program.procs.get(index).cloned().ok_or_else(|| {
            format!("{} call index {index} is out of range", self.program.name)
        })?;
        if proc.flags & 0x04 != 0 {
            return Err(format!(
                "{} imported procedure {} is not used by these Arroyo procedures",
                self.program.name, proc.name
            ));
        }
        self.ip = proc.body;
        if proc.flags & 0x10 != 0 {
            self.flags |= F_CRITICAL;
        }
        Ok(())
    }

    fn op_pop_flags(&mut self) -> Result<(), String> {
        let _window = self.pop_int()?;
        let _wait = self.pop()?;
        self.flags = (self.pop_int()? as u32) & 0xFFFF;
        Ok(())
    }

    fn op_fetch_global(&mut self) -> Result<(), String> {
        let addr = self.pop_int()?;
        let pos = self.base + addr;
        if pos < 0 || pos as usize >= self.stack.len() {
            return Err(format!("{} fetch global {pos} is out of range", self.program.name));
        }
        let value = self.stack[pos as usize];
        self.push(value);
        Ok(())
    }

    fn op_store_global(&mut self) -> Result<(), String> {
        let addr = self.pop_int()?;
        let value = self.pop()?;
        let pos = self.base + addr;
        if pos < 0 {
            return Err(format!("{} store global {pos} is negative", self.program.name));
        }
        let index = pos as usize;
        if index >= self.stack.len() {
            self.stack.resize(index + 1, Val::int(0));
        }
        self.stack[index] = value;
        Ok(())
    }

    fn op_store(&mut self, _global: bool) -> Result<(), String> {
        let addr = self.pop_int()?;
        let value = self.pop()?;
        let pos = self.frame + addr;
        if pos < 0 {
            return Err(format!("{} store {pos} is negative", self.program.name));
        }
        let index = pos as usize;
        if index >= self.stack.len() {
            self.stack.resize(index + 1, Val::int(0));
        }
        self.stack[index] = value;
        Ok(())
    }

    fn op_fetch(&mut self, _global: bool) -> Result<(), String> {
        let addr = self.pop_int()?;
        let pos = self.frame + addr;
        if pos < 0 || pos as usize >= self.stack.len() {
            return Err(format!("{} fetch {pos} is out of range", self.program.name));
        }
        let value = self.stack[pos as usize];
        self.push(value);
        Ok(())
    }

    fn op_fetch_external(&mut self, host: &mut Host) -> Result<(), String> {
        let addr = self.pop()?.as_int();
        let name = self.program.ident(addr);
        let value = *host.env.extern_vars.get(&name).ok_or_else(|| {
            format!("{} external variable {name} does not exist", self.program.name)
        })?;
        self.push(value);
        Ok(())
    }

    fn op_store_external(&mut self, host: &mut Host) -> Result<(), String> {
        let addr = self.pop()?.as_int();
        let value = self.pop()?;
        let name = self.program.ident(addr);
        if !host.env.extern_vars.contains_key(&name) {
            return Err(format!("{} external variable {name} does not exist", self.program.name));
        }
        host.env.extern_vars.insert(name, value);
        Ok(())
    }

    fn op_export_var(&mut self, host: &mut Host) -> Result<(), String> {
        let addr = self.pop()?.as_int();
        let name = self.program.ident(addr);
        if host.env.extern_vars.contains_key(&name) {
            return Err(format!("external variable {name} already exists"));
        }
        host.env.extern_vars.insert(name, Val::int(0));
        Ok(())
    }

    fn op_export_proc(&mut self, host: &mut Host) -> Result<(), String> {
        let _index = self.pop_int()?;
        let _argc = self.pop_int()?;
        let _ = host;
        Ok(())
    }

    fn op_check_argc(&mut self) -> Result<(), String> {
        let expected = self.pop_int()?;
        let index = self.pop_int()? as usize;
        let proc = self.program.procs.get(index).ok_or_else(|| {
            format!("{} argc check index {index} is out of range", self.program.name)
        })?;
        if proc.argc != expected {
            return Err(format!(
                "wrong number of args to procedure {}: expected {expected}, procedure says {}",
                proc.name, proc.argc
            ));
        }
        Ok(())
    }

    fn op_lookup(&mut self) -> Result<(), String> {
        let value = self.pop()?;
        let name = self.text(value);
        let found = self
            .program
            .procs
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, proc)| proc.name.eq_ignore_ascii_case(&name));
        if let Some((index, _)) = found {
            self.push(Val::int(index as i32));
            Ok(())
        } else {
            Err(format!("{} lookup did not find procedure {name}", self.program.name))
        }
    }

    fn op_cmp(&mut self, _host: &mut Host, cmp: Cmp) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = if matches!(left.kind, Kind::Str | Kind::Dyn) || matches!(right.kind, Kind::Str | Kind::Dyn) {
            let ordering = self.text(left).cmp(&self.text(right));
            match cmp {
                Cmp::Eq => ordering.is_eq(),
                Cmp::Ne => ordering.is_ne(),
                Cmp::Lt => ordering.is_lt(),
                Cmp::Gt => ordering.is_gt(),
                Cmp::Le => ordering.is_le(),
                Cmp::Ge => ordering.is_ge(),
            }
        } else if left.kind == Kind::Float || right.kind == Kind::Float {
            let l = if left.kind == Kind::Float { left.float } else { left.as_int() as f32 };
            let r = if right.kind == Kind::Float { right.float } else { right.as_int() as f32 };
            match cmp {
                Cmp::Eq => l == r,
                Cmp::Ne => l != r,
                Cmp::Lt => l < r,
                Cmp::Gt => l > r,
                Cmp::Le => l <= r,
                Cmp::Ge => l >= r,
            }
        } else {
            let (l, r) = (left.as_int(), right.as_int());
            match cmp {
                Cmp::Eq => l == r,
                Cmp::Ne => l != r,
                Cmp::Lt => l < r,
                Cmp::Gt => l > r,
                Cmp::Le => l <= r,
                Cmp::Ge => l >= r,
            }
        };
        self.push(Val::int(if result { 1 } else { 0 }));
        Ok(())
    }

    fn op_arith(&mut self, op: Arith) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        if op == Arith::Add && (matches!(left.kind, Kind::Str | Kind::Dyn) || matches!(right.kind, Kind::Str | Kind::Dyn)) {
            let text = format!("{}{}", self.text(left), self.text(right));
            let value = self.intern(text);
            self.push(value);
            return Ok(());
        }
        if left.kind == Kind::Float || right.kind == Kind::Float {
            let l = if left.kind == Kind::Float { left.float } else { left.as_int() as f32 };
            let r = if right.kind == Kind::Float { right.float } else { right.as_int() as f32 };
            let value = match op {
                Arith::Add => l + r,
                Arith::Sub => l - r,
                Arith::Mul => l * r,
                Arith::Div => {
                    if r == 0.0 { 0.0 } else { l / r }
                }
                Arith::Mod => {
                    if r == 0.0 { 0.0 } else { l % r }
                }
            };
            self.push(Val {
                kind: Kind::Float,
                bits: 0,
                float: value,
            });
            return Ok(());
        }
        let (l, r) = (left.as_int(), right.as_int());
        let value = match op {
            Arith::Add => l.wrapping_add(r),
            Arith::Sub => l.wrapping_sub(r),
            Arith::Mul => l.wrapping_mul(r),
            Arith::Div => {
                if r == 0 { 0 } else { l.wrapping_div(r) }
            }
            Arith::Mod => {
                if r == 0 { 0 } else { l.wrapping_rem(r) }
            }
        };
        self.push(Val::int(value));
        Ok(())
    }

    fn op_logic(&mut self, or: bool) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let value = if or {
            !left.empty() || !right.empty()
        } else {
            !left.empty() && !right.empty()
        };
        self.push(Val::int(if value { 1 } else { 0 }));
        Ok(())
    }

    fn op_bit(&mut self, op: Bit) -> Result<(), String> {
        let right = self.pop_int()?;
        let left = self.pop_int()?;
        let value = match op {
            Bit::And => left & right,
            Bit::Or => left | right,
            Bit::Xor => left ^ right,
        };
        self.push(Val::int(value));
        Ok(())
    }

    fn game(&mut self, index: u16, host: &mut Host) -> Result<(), String> {
        let name = self.program.name.clone();
        match index {
            0x0A1 => {
                host.env.xp = host.env.xp.saturating_add(self.pop_int()?);
                Ok(())
            }
            0x0A2 => {
                let _value = self.pop_int()?;
                Ok(())
            }
            0x0A4 => {
                let who = self.pop()?.as_int();
                let name = object_name(host, who);
                let value = self.intern(name);
                self.push(value);
                Ok(())
            }
            0x0A6 => {
                let stat = self.pop_int()?;
                self.push(Val::int(host.env.stat(stat)));
                Ok(())
            }
            0x0A8 | 0x0A9 => {
                let rotation = self.pop_int()?;
                let elevation = self.pop_int()?;
                let y = self.pop_int()?;
                let x = self.pop_int()?;
                let tile = 200 * y + x;
                if (0..HEX_COUNT).contains(&tile) {
                    host.env.player_tile = tile;
                    host.env.player_elevation = elevation;
                    host.env.player_facing = rotation.rem_euclid(6);
                    if let Some(dude) = host.env.actor_mut(Env::dude()) {
                        dude.tile = tile;
                        dude.elevation = elevation;
                        dude.facing = rotation.rem_euclid(6);
                    }
                }
                Ok(())
            }
            0x0AC => {
                let _mod = self.pop_int()?;
                let _skill = self.pop_int()?;
                let _who = self.pop()?;
                self.push(Val::int(2));
                Ok(())
            }
            0x0AF => {
                let roll = self.pop_int()?;
                let result = match roll {
                    0 | 1 => 0,
                    2 | 3 => 1,
                    _ => -1,
                };
                self.push(Val::int(result));
                Ok(())
            }
            0x0B0 => {
                let roll = self.pop_int()?;
                self.push(Val::int(if roll == 0 || roll == 3 { 1 } else { 0 }));
                Ok(())
            }
            0x0B4 => {
                let max = self.pop_int()?;
                let min = self.pop_int()?;
                let value = host.env.random(min, max);
                self.push(Val::int(value));
                Ok(())
            }
            0x0B6 => {
                let elevation = self.pop_int()?;
                let tile = self.pop_int()?;
                let who = self.pop()?.as_int();
                if who == 0 {
                    self.push(Val::int(-1));
                } else if who == Env::dude() {
                    host.env.player_tile = tile;
                    host.env.player_elevation = elevation;
                    if let Some(dude) = host.env.actor_mut(Env::dude()) {
                        dude.tile = tile;
                        dude.elevation = elevation;
                    }
                    self.push(Val::int(tile));
                } else if let Some(actor) = host.env.actor_mut(who) {
                    actor.tile = tile;
                    actor.elevation = elevation;
                    self.push(Val::int(tile));
                } else {
                    self.push(Val::int(-1));
                }
                Ok(())
            }
            0x0B8 => {
                let value = self.pop()?;
                let text = self.text(value);
                if !text.is_empty() {
                    host.env.transcript.lines.push(text);
                }
                Ok(())
            }
            0x0B9 => {
                host.env.overrides = 1;
                Ok(())
            }
            0x0BC => {
                self.push(Val::ptr(host.env.self_obj));
                Ok(())
            }
            0x0BD => {
                self.push(Val::ptr(host.env.source_obj));
                Ok(())
            }
            0x0BE => {
                self.push(Val::ptr(host.env.target_obj));
                Ok(())
            }
            0x0BF => {
                self.push(Val::ptr(Env::dude()));
                Ok(())
            }
            0x0C1 => {
                let index = self.pop_int()?;
                let value = host.env.locals.get(index as usize).copied().unwrap_or(-1);
                self.push(Val::int(value));
                Ok(())
            }
            0x0C2 => {
                let value = self.pop_int()?;
                let index = self.pop_int()?;
                if index >= 0 {
                    let slot = index as usize;
                    if slot >= host.env.locals.len() {
                        host.env.locals.resize(slot + 1, 0);
                    }
                    host.env.locals[slot] = value;
                }
                Ok(())
            }
            0x0C3 => {
                let index = self.pop_int()?;
                let value = if index >= 0 {
                    host.env.map_vars.get(index as usize).copied().unwrap_or(-1)
                } else {
                    -1
                };
                self.push(Val::int(value));
                Ok(())
            }
            0x0C4 => {
                let value = self.pop_int()?;
                let index = self.pop_int()?;
                if index >= 0 {
                    let slot = index as usize;
                    if slot >= host.env.map_vars.len() {
                        host.env.map_vars.resize(slot + 1, 0);
                    }
                    host.env.map_vars[slot] = value;
                }
                Ok(())
            }
            0x0C5 => {
                let index = self.pop_int()?;
                host.env.ensure_global(index)?;
                self.push(Val::int(host.env.globals[index as usize]));
                Ok(())
            }
            0x0C6 => {
                let value = self.pop_int()?;
                let index = self.pop_int()?;
                host.env.ensure_global(index)?;
                host.env.globals[index as usize] = value;
                Ok(())
            }
            0x0C7 => {
                self.push(Val::int(host.env.action));
                Ok(())
            }
            0x0C8 => {
                let who = self.pop()?.as_int();
                let kind = host.env.actor(who).map(|actor| match actor.kind {
                    "item" => 0,
                    "critter" => 1,
                    "scenery" => 2,
                    "wall" => 3,
                    "tile" => 4,
                    _ => 5,
                }).unwrap_or(-1);
                self.push(Val::int(kind));
                Ok(())
            }
            0x0CA => {
                let stat = self.pop_int()?;
                let _who = self.pop()?;
                self.push(Val::int(host.env.stat(stat)));
                Ok(())
            }
            0x0D4 => {
                let who = self.pop()?.as_int();
                let tile = if who == Env::dude() {
                    host.env.player_tile
                } else {
                    host.env.actor(who).map(|actor| actor.tile).unwrap_or(-1)
                };
                self.push(Val::int(tile));
                Ok(())
            }
            0x0D5 => {
                let distance = self.pop_int()?;
                let rotation = self.pop_int()?;
                let mut tile = self.pop_int()?;
                if tile != -1 && (0..6).contains(&rotation) && distance > 0 {
                    for _ in 0..distance {
                        tile = hexgrid::step(tile, rotation).unwrap_or(-1);
                        if !(0..HEX_COUNT).contains(&tile) {
                            tile = -1;
                            break;
                        }
                    }
                } else if distance == 0 {
                    tile = -1;
                }
                self.push(Val::int(tile));
                Ok(())
            }
            0x0DE => {
                for _ in 0..5 {
                    self.pop()?;
                }
                Ok(())
            }
            // end_dialogue and gsay_start pop nothing. gsay_end suspends the
            // procedure the way the engine's dialogue UI does: the reply and
            // options already recorded are the talk result.
            0x0DF | 0x11C => Ok(()),
            0x11D => {
                self.flags |= F_FINISHED;
                Ok(())
            }
            0x0E9 => {
                host.env.light = self.pop_int()?;
                Ok(())
            }
            0x0EA => {
                self.push(Val::int(host.env.ticks));
                Ok(())
            }
            0x0EB => {
                self.push(Val::int(host.env.ticks / 10));
                Ok(())
            }
            0x0EC => {
                let who = self.pop()?.as_int();
                let elevation = if who == 0 {
                    0
                } else if who == Env::dude() {
                    host.env.player_elevation
                } else {
                    host.env.actor(who).map(|actor| actor.elevation).unwrap_or(0)
                };
                self.push(Val::int(elevation));
                Ok(())
            }
            0x0FC => {
                let delta = self.pop_int()?;
                host.env.ticks = host.env.ticks.saturating_add(delta);
                host.env.hour = host.env.ticks.rem_euclid(864000) / 36000;
                Ok(())
            }
            0x0F0 => {
                let _param = self.pop()?;
                let _delay = self.pop()?;
                let _who = self.pop()?;
                Ok(())
            }
            0x0F2 => {
                let ticks = self.pop_int()?.max(0);
                self.push(Val::int(ticks.saturating_mul(10)));
                Ok(())
            }
            0x0F3 => {
                let _param = self.pop_int()?;
                let _who = self.pop()?;
                let _kind = self.pop_int()?;
                self.push(Val::int(0));
                Ok(())
            }
            0x0F6 => {
                self.push(Val::int(host.env.ticks.rem_euclid(864000) / 36000));
                Ok(())
            }
            0x0F7 => {
                self.push(Val::int(host.env.fixed_param));
                Ok(())
            }
            0x100 => {
                let who = self.pop()?.as_int();
                let pid = host.env.actor(who).map(|actor| actor.pid).unwrap_or(0);
                self.push(Val::int(pid));
                Ok(())
            }
            0x101 => {
                self.push(Val::int(0));
                Ok(())
            }
            0x102 => {
                let _value = self.pop_int()?;
                let _param = self.pop_int()?;
                let _kind = self.pop_int()?;
                let _who = self.pop()?;
                self.push(Val::int(-1));
                Ok(())
            }
            0x104 => {
                let _member = self.pop_int()?;
                let _pid = self.pop()?;
                self.push(Val::int(0));
                Ok(())
            }
            0x105 => {
                let message = self.pop_int()?;
                let file_id = self.pop_int()?;
                let text = host.message(file_id, message)?;
                let value = self.intern(text);
                self.push(value);
                Ok(())
            }
            0x10A => {
                let _kind = self.pop_int()?;
                let value = self.pop()?;
                let _who = self.pop()?;
                if matches!(value.kind, Kind::Str | Kind::Dyn) {
                    let text = self.text(value);
                    if !text.is_empty() {
                        host.env.transcript.lines.push(text);
                    }
                }
                Ok(())
            }
            0x10B => {
                let _param = self.pop()?;
                let rule = self.pop_int()?;
                let result = if rule == 14 {
                    i32::from(host.env.map_flags & 1 == 0)
                } else {
                    0
                };
                self.push(Val::int(result));
                Ok(())
            }
            0x10E => {
                let _param = self.pop()?;
                let _cmd = self.pop_int()?;
                Ok(())
            }
            0x10F | 0x110 => {
                let _delay = self.pop_int()?;
                let _anim = self.pop_int()?;
                let _who = self.pop()?;
                Ok(())
            }
            0x113 | 0x114 => {
                let _delay = self.pop_int()?;
                let _tile = self.pop_int()?;
                let _who = self.pop()?;
                Ok(())
            }
            0x115 => {
                let _movie = self.pop_int()?;
                Ok(())
            }
            0x118 => {
                self.push(Val::int(host.env.month));
                Ok(())
            }
            0x119 => {
                self.push(Val::int(1));
                Ok(())
            }
            0x11E => {
                let msg = self.pop()?;
                let list = self.pop_int()?;
                let text = self.dialog_text(host, list, msg)?;
                if !text.is_empty() {
                    host.env.transcript.replies.push(text);
                }
                Ok(())
            }
            0x11F => {
                let _reaction = self.pop_int()?;
                let proc = self.pop()?;
                let msg = self.pop()?;
                let list = self.pop_int()?;
                let text = self.dialog_text(host, list, msg)?;
                self.record_option(host, proc, text);
                Ok(())
            }
            0x120 => {
                let _reaction = self.pop_int()?;
                let msg = self.pop()?;
                let list = self.pop_int()?;
                let text = self.dialog_text(host, list, msg)?;
                if !text.is_empty() {
                    host.env.transcript.replies.push(text.clone());
                    host.env.transcript.lines.push(text);
                }
                Ok(())
            }
            0x121 => {
                let _reaction = self.pop_int()?;
                let proc = self.pop()?;
                let msg = self.pop()?;
                let list = self.pop_int()?;
                let iq = self.pop_int()?;
                let intelligence = host.env.stat(4);
                let show = if iq < 0 {
                    -intelligence >= iq
                } else {
                    intelligence >= iq
                };
                if show {
                    let text = self.dialog_text(host, list, msg)?;
                    self.record_option(host, proc, text);
                }
                Ok(())
            }
            0x12D => {
                let who = self.pop()?.as_int();
                let locked = host.env.actor(who).is_some_and(|actor| actor.locked);
                self.push(Val::int(if locked { 1 } else { 0 }));
                Ok(())
            }
            0x12E | 0x12F => {
                let who = self.pop()?.as_int();
                let lock = index == 0x12E;
                if let Some(actor) = host.env.actor_mut(who) {
                    actor.locked = lock;
                }
                Ok(())
            }
            0x154 => {
                let value = self.pop()?;
                let text = self.text(value);
                if !text.is_empty() && matches!(value.kind, Kind::Str | Kind::Dyn) {
                    host.env.transcript.lines.push(text);
                }
                Ok(())
            }
            // Sound-name builders always push a string. An empty name is not displayed.
            0x0A5 => self.pop_push_empty(2),
            0x13D => self.pop_push_empty(3),
            0x13E | 0x13F | 0x140 => self.pop_push_empty(1),
            0x141 => self.pop_push_empty(4),
            0x142 => self.pop_push_empty(3),
            // do_check rolls a stat. The play loop is not in combat and the dude's
            // SPECIAL is fixed, so the roll is the same success `roll_vs_skill` returns.
            0x0AE => {
                for _ in 0..3 {
                    self.pop()?;
                }
                self.push(Val::int(2));
                host.env.note_stub(0x0AE);
                Ok(())
            }
            0x0CF => {
                let mut tiles = [0; 5];
                for tile in &mut tiles {
                    *tile = self.pop_int()?;
                }
                let x = tiles[0] % 200;
                let y = tiles[0] / 200;
                let min_x = tiles[1] % 200;
                let max_x = tiles[4] % 200;
                let min_y = tiles[4] / 200;
                let max_y = tiles[1] / 200;
                let inside = x >= min_x && x <= max_x && y >= min_y && y <= max_y;
                self.push(Val::int(i32::from(inside)));
                Ok(())
            }
            0x0D0 | 0x0DD => {
                // attack pops seven integers and the target, then requests combat.
                // Combat is not simulated; the arguments are consumed so the procedure continues.
                for _ in 0..8 {
                    self.pop()?;
                }
                host.env.note_stub(index);
                Ok(())
            }
            0x0D2 => {
                let tile2 = self.pop_int()?;
                let tile1 = self.pop_int()?;
                self.push(Val::int(hexgrid::distance(tile1, tile2)));
                Ok(())
            }
            0x0D3 => {
                let second = self.pop()?.as_int();
                let first = self.pop()?.as_int();
                let distance = match (self.object_place(host, first), self.object_place(host, second)) {
                    (Some((tile1, elev1)), Some((tile2, elev2))) if elev1 == elev2 => {
                        hexgrid::distance(tile1, tile2)
                    }
                    _ => 9999,
                };
                self.push(Val::int(distance));
                Ok(())
            }
            0x103 => {
                for _ in 0..4 {
                    self.pop()?;
                }
                self.push(Val::int(-1));
                Ok(())
            }
            0x14C => {
                let tile2 = self.pop_int()?;
                let tile1 = self.pop_int()?;
                self.push(Val::int(hexgrid::rotation_to(tile1, tile2)));
                Ok(())
            }
            other => {
                // Straight-line opcodes from the engine: pop the arguments, and
                // push 0 when the opcode always returns a value. Combat and
                // animation effects are not simulated.
                if let Some((pops, push)) = stub_stack(other) {
                    for _ in 0..pops {
                        self.pop()?;
                    }
                    if push {
                        self.push(Val::int(0));
                    }
                    host.env.note_stub(other);
                    return Ok(());
                }
                Err(format!(
                    "{name} unimplemented opcode {:#x} at ip {:#x}",
                    0x8000 | other,
                    self.ip
                ))
            }
        }
    }

    fn pop_push_empty(&mut self, pops: usize) -> Result<(), String> {
        for _ in 0..pops {
            self.pop()?;
        }
        let value = self.intern(String::new());
        self.push(value);
        Ok(())
    }

    fn object_place(&self, host: &Host, handle: i32) -> Option<(i32, i32)> {
        if handle == 0 {
            return None;
        }
        if handle == Env::dude() {
            return Some((host.env.player_tile, host.env.player_elevation));
        }
        host.env.actor(handle).map(|actor| (actor.tile, actor.elevation))
    }

    fn dialog_text(&mut self, host: &mut Host, list: i32, msg: Val) -> Result<String, String> {
        if matches!(msg.kind, Kind::Str | Kind::Dyn) {
            Ok(self.text(msg))
        } else {
            host.message(list, msg.as_int())
        }
    }

    fn proc_label(&self, proc: Val) -> String {
        if matches!(proc.kind, Kind::Str | Kind::Dyn) {
            self.text(proc)
        } else {
            self.program
                .procs
                .get(proc.as_int() as usize)
                .map(|proc| proc.name.clone())
                .unwrap_or_default()
        }
    }

    fn record_option(&mut self, host: &mut Host, proc: Val, text: String) {
        if text.is_empty() {
            return;
        }
        let procedure = self.proc_label(proc);
        let script_index = host
            .scripts
            .iter()
            .position(|script| script.name.eq_ignore_ascii_case(&self.program.name))
            .map(|index| index as i32)
            .unwrap_or(-1);
        host.env.transcript.options.push(DialogueOption {
            text,
            procedure,
            script_index,
            self_handle: host.env.self_obj,
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp { Eq, Ne, Lt, Gt, Le, Ge }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arith { Add, Sub, Mul, Div, Mod }
#[derive(Clone, Copy)]
enum Bit { And, Or, Xor }

pub struct Host<'a> {
    pub files: &'a GameFiles,
    pub scripts: &'a [ScriptName],
    pub env: &'a mut Env,
}

impl Host<'_> {
    fn message(&mut self, file_id: i32, message_id: i32) -> Result<String, String> {
        if message_id < 0 {
            return Ok("Error".into());
        }
        if !self.env.msg_cache.contains_key(&file_id) {
            let (path, table) = load_messages(self.files, self.scripts, file_id)?;
            self.env.msg_path.insert(file_id, path);
            self.env.msg_cache.insert(file_id, table);
        }
        let path = self.env.msg_path.get(&file_id).cloned().unwrap_or_default();
        if !path.is_empty() && !self.env.transcript.message_paths.iter().any(|have| have == &path) {
            self.env.transcript.message_paths.push(path.clone());
        }
        let text = self
            .env
            .msg_cache
            .get(&file_id)
            .and_then(|table| table.get(&message_id))
            .cloned()
            .unwrap_or_else(|| "Error".into());
        if text != "Error" && !self.env.transcript.message_texts.iter().any(|have| have == &text) {
            self.env.transcript.message_texts.push(text.clone());
        }
        Ok(text)
    }
}

/// Stack effect of a straight-line game opcode that this play loop does not simulate.
/// `(pops, push_zero)`. Opcodes with a real arm in `game` are absent.
fn stub_stack(index: u16) -> Option<(u8, bool)> {
    Some(match index {
        0x0A3 => (1, false),
        0x0A7 => (3, true),
        0x0AA => (2, true),
        0x0AB => (2, true),
        0x0AD => (3, true),
        0x0B1 => (1, true),
        0x0B2 => (3, false),
        0x0B3 => (3, true),
        0x0B5 => (2, true),
        0x0B7 => (4, true),
        0x0BA => (2, true),
        0x0BB => (3, true),
        0x0C0 => (0, true),
        0x0C9 => (1, true),
        0x0CB => (3, true),
        0x0CC => (1, false),
        0x0CD => (1, false),
        0x0CE => (3, false),
        0x0D1 => (0, false),
        0x0D6 => (1, false),
        0x0D7 => (1, false),
        0x0D8 => (2, false),
        0x0D9 => (2, false),
        0x0DA => (2, false),
        0x0DB => (1, false),
        0x0DC => (2, true),
        0x0E0 => (1, false),
        0x0E1 => (4, true),
        0x0E2 => (2, false),
        0x0E3 => (2, false),
        0x0E4 => (2, false),
        0x0E5 => (3, false),
        0x0E6 => (5, false),
        0x0E7 => (1, true),
        0x0E8 => (2, true),
        0x0ED => (2, false),
        0x0EE => (2, false),
        0x0EF => (3, false),
        0x0F1 => (1, false),
        0x0F4 => (1, false),
        0x0F5 => (2, true),
        0x0F8 => (1, true),
        0x0F9 => (0, false),
        0x0FA => (0, true),
        0x0FB => (1, true),
        0x0FD => (2, false),
        0x0FE => (2, false),
        0x0FF => (3, true),
        0x106 => (2, true),
        0x107 => (3, false),
        0x108 => (0, false),
        0x109 => (3, true),
        0x10C => (3, false),
        0x10D => (2, true),
        0x111 => (3, false),
        0x112 => (3, false),
        0x116 => (3, false),
        0x117 => (3, true),
        0x11A => (3, false),
        0x11B => (0, true),
        0x122 => (2, false),
        0x123 => (1, true),
        0x124 => (1, false),
        0x125 => (1, false),
        0x126 => (2, false),
        0x127 => (2, false),
        0x128 => (0, true),
        0x129 => (1, false),
        0x12A => (0, true),
        0x12B => (0, true),
        0x12C => (0, false),
        0x130 => (1, true),
        0x131 => (1, false),
        0x132 => (1, false),
        0x133 => (0, false),
        0x134 => (0, false),
        0x135 => (0, true),
        0x136 => (1, false),
        0x137 => (1, false),
        0x138 => (1, true),
        0x139 => (2, true),
        0x13A => (2, true),
        0x13B => (3, false),
        0x13C => (3, false),
        0x143 => (2, false),
        0x144 => (2, true),
        0x145 => (2, false),
        0x146 => (0, false),
        0x147 => (2, false),
        0x148 => (0, false),
        0x149 => (1, true),
        0x14A => (1, true),
        0x14B => (1, true),
        0x14D => (1, false),
        0x14E => (1, false),
        0x14F => (0, true),
        0x150 => (1, true),
        0x151 => (1, true),
        0x152 => (2, false),
        0x153 => (0, false),
        0x155 => (1, false),
        _ => return None,
    })
}

fn load_messages(files: &GameFiles, scripts: &[ScriptName], file_id: i32) -> Result<(String, HashMap<i32, String>), String> {
    if file_id <= 0 || file_id as usize > scripts.len() {
        return Ok((String::new(), HashMap::new()));
    }
    let name = &scripts[file_id as usize - 1].name;
    let candidates = [
        format!("text\\english\\dialog\\{name}.msg"),
        format!("text\\english\\game\\{name}.msg"),
    ];
    for path in candidates {
        if !files.contains(&path) {
            continue;
        }
        let bytes = files.read(&path)?;
        return Ok((path, parse_msg(&bytes)));
    }
    Ok((String::new(), HashMap::new()))
}

fn parse_msg(bytes: &[u8]) -> HashMap<i32, String> {
    // A message is three brace fields. The text field may contain newlines.
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    let mut table = HashMap::new();
    let mut fields = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        i += 1;
        let mut depth = 1i32;
        let mut current = String::new();
        while i < chars.len() && depth > 0 {
            match chars[i] {
                '{' => {
                    depth += 1;
                    if depth > 1 {
                        current.push('{');
                    }
                }
                '}' => {
                    depth -= 1;
                    if depth > 0 {
                        current.push('}');
                    }
                }
                ch => current.push(ch),
            }
            i += 1;
        }
        fields.push(current);
        if fields.len() == 3 {
            if let Ok(id) = fields[0].trim().parse::<i32>() {
                table.insert(id, fields[2].clone());
            }
            fields.clear();
        }
    }
    table
}

fn note_path(kind: i32) -> Option<&'static str> {
    Some(match kind {
        0 => "text\\english\\game\\pro_item.msg",
        1 => "text\\english\\game\\pro_crit.msg",
        2 => "text\\english\\game\\pro_scen.msg",
        3 => "text\\english\\game\\pro_wall.msg",
        5 => "text\\english\\game\\pro_misc.msg",
        _ => return None,
    })
}

/// Short proto name. Critter 1 is "Hero Male" in `pro_crit.msg` (message 100).
fn object_name(host: &mut Host, handle: i32) -> String {
    if handle == 0 {
        return String::new();
    }
    // The dude is the male hero proto until character creation stores another name.
    let pid = if handle == Env::dude() {
        0x0100_0001
    } else {
        host.env.actor(handle).map(|actor| actor.pid).unwrap_or(0)
    };
    let kind = (pid >> 24) & 0xff;
    let index = pid & 0x00ff_ffff;
    let Some(path) = note_path(kind) else {
        return String::new();
    };
    let cache_key = -10 - kind;
    if !host.env.msg_cache.contains_key(&cache_key) {
        let table = match host.files.read(path) {
            Ok(bytes) => parse_msg(&bytes),
            Err(_) => HashMap::new(),
        };
        host.env.msg_cache.insert(cache_key, table);
    }
    host.env
        .msg_cache
        .get(&cache_key)
        .and_then(|table| table.get(&(index * 100)))
        .cloned()
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}

pub fn run_procedure(
    program: &Program,
    host: &mut Host,
    proc_name: &str,
    init: bool,
) -> Result<bool, String> {
    if !program.has_proc(proc_name) {
        return Ok(false);
    }
    let mut machine = Machine::new(program.clone());
    if init {
        machine.interpret(host, true)?;
        if machine.flags & F_FATAL != 0 {
            return Err(format!("{} init failed", program.name));
        }
    }
    let index = machine.program.proc_index(proc_name).expect("proc");
    let body = machine.program.procs[index].body;
    host.env.transcript.procedures.push(proc_name.to_string());
    machine.flags &= !F_EXITED;
    machine.setup_call(body, RETURN_EXIT);
    machine.interpret(host, true)?;
    Ok(true)
}

fn be_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn cstr_at(data: &[u8], offset: usize) -> String {
    if offset >= data.len() {
        return String::new();
    }
    let end = data[offset..].iter().position(|byte| *byte == 0).unwrap_or(0);
    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
}
