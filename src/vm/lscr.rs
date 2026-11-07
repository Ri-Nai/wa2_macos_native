//! LSCR (`.bnr`) scenario bytecode: parser + interpreter.
//!
//! File layout (reverse-engineered from the shipped scripts):
//!   u32 magic = "LSCR"
//!   u32 header_size      (offset where the label table ends; 20 or 68...)
//!   u32 label_count
//!   (u32 label_id, u32 offset) * label_count   -- label 0 = main entry
//!   ...op stream...
//!
//! The op stream is a flat sequence of words. Decoded op forms:
//!   `6, kind`          record start (kind 30 = normal page, 0/8/16/27 = structural)
//!   `3, N`             token reference (index into the companion `.txt` token pool)
//!   `4, X`             page action (131 = display text, 144 = speaker,
//!                      172 = set background, 208 = choice option, 132 = wait, ...)
//!   `5, 3, V` / `5, 4, V`  int / float value
//!   `0, 12, T`         goto T
//!   `0, 2, A, B`       branch: option 1 -> fall through, option 2 -> A, B = merge
//!   `0, 4`             case-merge marker: when the preceding branch was taken as
//!                      option 1, jump to its B target (skip option 2's content)
//!   `0, 1`             return / script end
//!   `0, 13`            block end (no-op for playback)
//!   `1, sub` / `2, sub`  variable / runtime references; indexed globals are
//!                          persisted and equality conditions feed Branch
//!
//! The interpreter executes ops and raises `Event`s for the frontend to render.

use crate::codec::sjis;
use crate::vm::global::GlobalState;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Op {
    Rec { off: u32, kind: u32 },
    Tok { off: u32, n: u32 },
    Act { off: u32, id: u32 },
    Int { off: u32, v: i32 },
    Flt { off: u32, v: f32 },
    Goto { off: u32, target: u32 },
    Return { off: u32 },
    Branch { off: u32, a: u32, b: u32 },
    CaseEnd { off: u32 },
    Switch { off: u32, targets: Vec<u32> },
    BlockEnd { off: u32 },
    Var { off: u32, sub: u32 },
    Fun { off: u32, sub: u32 },
    Raw { off: u32, w: u32 },
}

pub fn op_off(op: &Op) -> u32 {
    match op {
        Op::Rec { off, .. }
        | Op::Tok { off, .. }
        | Op::Act { off, .. }
        | Op::Int { off, .. }
        | Op::Flt { off, .. }
        | Op::Goto { off, .. }
        | Op::Return { off }
        | Op::Branch { off, .. }
        | Op::CaseEnd { off }
        | Op::Switch { off, .. }
        | Op::BlockEnd { off }
        | Op::Var { off, .. }
        | Op::Fun { off, .. }
        | Op::Raw { off, .. } => *off,
    }
}

pub struct Program {
    pub ops: Vec<Op>,
    /// byte offset -> op index (jump targets land on op starts)
    pub index: HashMap<u32, usize>,
    pub labels: Vec<(u32, u32)>,
    /// the scenario token pool (comma-separated text in the companion `.txt`)
    pub tokens: Vec<String>,
    pub entry: u32,
}

fn rd(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Program {
    /// `cn` selects the Chinese-patch reading of the token pool: the patch
    /// keeps Shift-JIS codes and repaints the glyphs, so the bytes only mean
    /// something once they are looked up in its glyph table.
    pub fn load(bnr: &[u8], txt: &[u8], cn: bool) -> Result<Program, String> {
        if bnr.len() < 20 || &bnr[0..4] != b"LSCR" {
            return Err("not an LSCR file".into());
        }
        let count = rd(bnr, 8) as usize;
        let mut labels = Vec::new();
        for k in 0..count {
            let b = 12 + k * 8;
            if b + 8 > bnr.len() {
                break;
            }
            labels.push((rd(bnr, b), rd(bnr, b + 4)));
        }
        let entry = labels
            .iter()
            .find(|(id, _)| *id == 0)
            .or_else(|| labels.first())
            .map(|(_, o)| *o)
            .unwrap_or(28);

        let tokens: Vec<String> = txt
            .split(|&b| b == b',')
            .map(|t| if cn { sjis::decode_cn(t) } else { sjis::decode(t) })
            .collect();

        let mut ops = Vec::new();
        let n = bnr.len();
        let mut i = entry as usize;
        while i + 4 <= n {
            let off = i as u32;
            let op = rd(bnr, i);
            match op {
                6 => {
                    if i + 8 > n {
                        break;
                    }
                    ops.push(Op::Rec { off, kind: rd(bnr, i + 4) });
                    i += 8;
                }
                5 => {
                    if i + 12 > n {
                        break;
                    }
                    let t = rd(bnr, i + 4);
                    let v = rd(bnr, i + 8);
                    if t == 4 {
                        ops.push(Op::Flt { off, v: f32::from_bits(v) });
                    } else {
                        ops.push(Op::Int { off, v: v as i32 });
                    }
                    i += 12;
                }
                3 => {
                    if i + 8 > n {
                        break;
                    }
                    ops.push(Op::Tok { off, n: rd(bnr, i + 4) });
                    i += 8;
                }
                4 => {
                    if i + 8 > n {
                        break;
                    }
                    ops.push(Op::Act { off, id: rd(bnr, i + 4) });
                    i += 8;
                }
                0 => {
                    let sub = rd(bnr, i + 4);
                    match sub {
                        12 => {
                            if i + 12 > n {
                                break;
                            }
                            ops.push(Op::Goto { off, target: rd(bnr, i + 8) });
                            i += 12;
                        }
                        2 => {
                            if i + 16 > n {
                                break;
                            }
                            ops.push(Op::Branch { off, a: rd(bnr, i + 8), b: rd(bnr, i + 12) });
                            i += 16;
                        }
                        1 => {
                            ops.push(Op::Return { off });
                            i += 8;
                        }
                        4 => {
                            ops.push(Op::CaseEnd { off });
                            i += 8;
                        }
                        13 => {
                            ops.push(Op::BlockEnd { off });
                            i += 8;
                        }
                        5 | 14 => {
                            // multi-target switch: args are offset-like words
                            let mut j = i + 8;
                            let mut targets = Vec::new();
                            while j + 4 <= n && targets.len() < 8 {
                                let v = rd(bnr, j);
                                if v > 64 && (v as usize) < n {
                                    targets.push(v);
                                    j += 4;
                                } else {
                                    break;
                                }
                            }
                            ops.push(Op::Switch { off, targets });
                            i = j;
                        }
                        _ => {
                            ops.push(Op::Raw { off, w: sub });
                            i += 8;
                        }
                    }
                }
                1 | 2 => {
                    if i + 8 > n {
                        break;
                    }
                    let sub = rd(bnr, i + 4);
                    ops.push(if op == 1 { Op::Var { off, sub } } else { Op::Fun { off, sub } });
                    i += 8;
                }
                _ => {
                    ops.push(Op::Raw { off, w: op });
                    i += 4;
                }
            }
        }

        let mut index = HashMap::new();
        for (k, o) in ops.iter().enumerate() {
            index.entry(op_off(o)).or_insert(k);
        }
        Ok(Program { ops, index, labels, tokens, entry })
    }

    pub fn token(&self, n: u32) -> Option<&str> {
        self.tokens.get(n as usize).map(|s| s.as_str())
    }
}

/// Explicit voice-bank and clip identifiers carried by ACT137 / ACT138.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceCue {
    pub scenario: u32,
    pub line: u32,
    pub char_id: u32,
}

/// Events raised by the interpreter; the frontend renders them.
#[derive(Debug, Clone)]
pub enum Event {
    /// Display a text token (speaker name optional; voice identifiers from ACT138).
    Text { text: String, speaker: Option<String>, voice: Option<VoiceCue> },
    /// Scene background change (ACT146/ACT147).
    ///
    /// The record is `K30/K23 vals=(mode, id, variant, frames, 0, 0, 0,
    /// 1280.0, 720.0)`; the image is `b{id:04}{variant:02}.tga` with a
    /// `b{id:04}*` fallback. `id == 0` or `-2` means a black screen.
    /// `frames` is the transition length at 60 fps. The raw `mode` is kept
    /// only for K30 records: values >= 0x80 select the full-screen `f0NNN`
    /// effect plate, while structural K23 records use zero.
    BgScene { id: i64, variant: u32, frames: u32, mode: i32 },
    /// Event CG (ACT148), and a layer of one (ACT201 with the layer turned on).
    ///
    /// Same record shape as a background, but the exe's mode-1 path formats
    /// `v%06d.tga` from `id * 10 + variant` (10170, variant 0 → `v101700.tga`).
    /// It replaces the standing shot. `frames` is the crossfade at 60 fps;
    /// a layer switch passes 0.
    EventCg { id: i64, variant: u32, frames: u32 },
    /// ACT227: the same picture as an `EventCg`, but the record that describes
    /// it sits in front of the call instead of inside it.
    ///
    /// This is the scroll CG. A handful of event pictures are several screens
    /// tall (`v201900` is 1280×2160, `v200000` is 1280×3362) and the original
    /// drags them up through the 720-px frame; the short ones behave exactly
    /// like an `EventCg`. Nothing was handling the opcode at all, which is why
    /// the cut to black before one of them never turned into a picture.
    ///
    /// `pages` is the record's own trailing count of screenfuls the picture
    /// travels (`... 0, 0, 720, 2` for v201900, `720, 3` for v200000). It is
    /// 0 for every short picture, which is what makes them behave like an
    /// `EventCg`.
    ScrollCg { id: i64, variant: u32, frames: u32, pages: u32 },
    /// ACT152 `(subtype, frames, r, g, b)`. The screen snaps to that colour
    /// and fades back to the picture over `frames` at 60 fps. White is the
    /// climax flash; the subtype is unused (the snap itself is instant).
    Flash { frames: u32, r: u8, g: u8, b: u8 },
    /// Slot-based overlay graphic (ACT176): items, effect plates and layered-CG
    /// pieces. This is *not* a background change.
    Overlay { name: String, slot: u32 },
    /// Timed pause (ACT194), in frames at 60 fps.
    WaitFrames(u32),
    /// Original executable audio commands, with their raw integer arguments.
    Sound { opcode: u16, args: Vec<i32> },
    /// Colour filter over the whole screen (`sepia.AMP`, `nega.amp`, or empty
    /// to clear it). ACT172 only ever carries these - it is *not* a background
    /// op, and treating its empty token as "clear the background" wiped the
    /// background every time a scene dropped its tint.
    Tint { name: String },
    /// ACT171 updates the two in-game message-window alpha channels used by
    /// the original compositor. `channel=0` writes both channels;
    /// `channel=1` writes the novel window and `channel=2` the visual/CG
    /// window. Values are kept raw because the exe accepts a 32-bit field.
    WindowAlpha { value: i32, channel: i32 },
    /// Play a movie (token like `mv01`).
    Movie { name: String },
    /// Load/switch a character's standing sprite. The file name is
    /// `{prefix}{series:03}{variant:03}.tga` inside `char.pak`.
    ///
    /// `frames` is the crossfade length at 60 fps. ACT154 carries it in the
    /// record; ACT155 (the entrance) has none, so it uses 15.
    Sprite {
        char_id: u32,
        series: u32,
        variant: u32,
        slot: u32,
        frames: u32,
        kind: SpriteKind,
    },
    /// ACT156 `(char_id, 0, frames)`: that character leaves. `frames` is the
    /// exit length at 60 fps.
    SpriteExit { char_id: u32, frames: u32 },
    /// A character is now speaking (keep bright, dim the others).
    SpriteFocus { char_id: u32 },
    /// ACT170: ambient light tint preset. 0 clears it, 1 evening, 2 night,
    /// 3 indoor. The original executable's handler (dispatch table 0x4c25c0,
    /// index 62 -> 0x4555a0) reads the record's first int into a global; the
    /// renderer then picks the colour curve from the preset table at 0x4be328
    /// ([0] none, [1] "evening.amp", [2] "night.amp", [3] "indoor.amp"). It
    /// has nothing to do with the message window.
    EnvTint { preset: i32 },
    /// Present a choice; call `set_choice_result` with 1..=options.len().
    Choice { options: Vec<String> },
    /// Wait for the player to advance.
    Wait,
    /// Jump to another scenario (`CATCH` / `CATCH2` / `CATCH3`).
    Jump { target: String, kind: String },
    /// End of script.
    End,
}

/// K16/ACT155 is a new entrance; K16/ACT154 changes the expression of a
/// character already occupying a stage slot. Keeping the distinction in the
/// event stream lets the renderer match their transition semantics without
/// changing the decoded sprite payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteKind {
    Enter,
    Change,
}

pub struct Interp<'a> {
    pub p: &'a Program,
    /// Story-wide variables live outside a scene and are shared by every
    /// interpreter created while following CATCH jumps.
    pub global: &'a mut GlobalState,
    pub pc: usize,
    tok: Option<u32>,
    ints: Vec<i32>,
    flts: Vec<f32>,
    speaker: Option<(String, u32)>,
    pending_voice: Option<VoiceCue>,
    voice_scenario: u32,
    options: Vec<String>,
    choice_result: u32,
    pending_b: Option<u32>,
    last_kind: String,
    steps: u64,
    /// current record tracking (for sprite-load detection)
    rec_kind: u32,
    rec_tok_hit: bool,
    rec_act_hit: bool,
    /// Every token referenced inside the current record, in order.
    ///
    /// ACT176 ("load graphic from <dir>") carries its arguments as
    /// (slot, name, x, y, ?, alpha, dir, ?) - the image name is the first token
    /// and is followed by an empty token and the directory token, so the single
    /// `tok` field (which keeps the *last* token) is not enough.
    rec_toks: Vec<u32>,
    /// `K30` record that selected a sprite face, waiting for the following
    /// `K16` record (ACT154/155) to say *where* it goes on screen.
    pending_sprite: Option<(u32, u32, u32)>,
    /// The current record's global reference.  The bytecode form used by the
    /// port is `Var <index> Int <value>` for assignment and
    /// `Var <index> Fun 0 Int <value>` for an equality test consumed by the
    /// following Branch.
    rec_var: Option<u32>,
    rec_fun: Option<u32>,
    condition_result: Option<bool>,
    /// When a `Choice` event is presented, this holds the op index of the
    /// `Fun{sub=0}` instruction that raised it. A save made while the choices
    /// are on screen rewinds `pc` back here (and keeps `options`) so a load
    /// re-presents the same choice instead of skipping past it: `next_event`
    /// has already advanced `pc` past the option instruction by the time the
    /// frontend shows the menu, so without this the reloaded save starts after
    /// the choice with a stale/default `choice_result`.
    choice_pc: Option<usize>,
}

/// Everything about an interpreter that has to survive a save/load round trip.
///
/// `Interp` also borrows the `Program`; that is reloaded from the script files
/// when a save is loaded (`load_program`), so it is deliberately not part of
/// this. Every field here is plain data, and `pc` is an index into
/// `Program::ops`, which is rebuilt identically on every load.
#[derive(Clone, Debug)]
pub struct InterpState {
    pub pc: usize,
    pub tok: Option<u32>,
    pub ints: Vec<i32>,
    pub flts: Vec<f32>,
    pub speaker: Option<(String, u32)>,
    pub pending_voice: Option<VoiceCue>,
    pub voice_scenario: u32,
    pub options: Vec<String>,
    /// The answer to the last choice. `Branch`/`Switch` fall through to option
    /// 1 whenever this is 1, so losing it would silently reroute the scene.
    pub choice_result: u32,
    /// Merge target of the branch being executed. Saved verbatim: `Branch` is
    /// written far more often than `CaseEnd` consumes it, so a stale value is
    /// the normal state, not a bug.
    pub pending_b: Option<u32>,
    pub last_kind: String,
    pub steps: u64,
    pub rec_kind: u32,
    pub rec_tok_hit: bool,
    pub rec_act_hit: bool,
    pub rec_toks: Vec<u32>,
    pub pending_sprite: Option<(u32, u32, u32)>,
    pub rec_var: Option<u32>,
    pub rec_fun: Option<u32>,
    pub condition_result: Option<bool>,
    pub choice_pc: Option<usize>,
}

impl<'a> Interp<'a> {
    pub fn snapshot(&self) -> InterpState {
        // A save taken while a choice menu is on screen must rewind to the
        // `Fun{sub=0}` that raised it and keep the option text, otherwise a
        // load resumes *after* the choice with a stale/default answer and the
        // player never sees the menu again.
        let (pc, options) = match self.choice_pc {
            Some(rewind_pc) => (rewind_pc, self.options.clone()),
            None => (self.pc, self.options.clone()),
        };
        InterpState {
            pc,
            tok: self.tok,
            ints: self.ints.clone(),
            flts: self.flts.clone(),
            speaker: self.speaker.clone(),
            pending_voice: self.pending_voice,
            voice_scenario: self.voice_scenario,
            options,
            choice_result: self.choice_result,
            pending_b: self.pending_b,
            last_kind: self.last_kind.clone(),
            steps: self.steps,
            rec_kind: self.rec_kind,
            rec_tok_hit: self.rec_tok_hit,
            rec_act_hit: self.rec_act_hit,
            rec_toks: self.rec_toks.clone(),
            pending_sprite: self.pending_sprite,
            rec_var: self.rec_var,
            rec_fun: self.rec_fun,
            condition_result: self.condition_result,
            choice_pc: None,
        }
    }

    /// Put the interpreter back where a save left it. The caller is responsible
    /// for handing back a `Program` for the same script, since the ops behind
    /// `pc` come from there.
    pub fn restore(&mut self, s: InterpState) {
        self.pc = s.pc;
        self.tok = s.tok;
        self.ints = s.ints;
        self.flts = s.flts;
        self.speaker = s.speaker;
        self.pending_voice = s.pending_voice;
        self.voice_scenario = s.voice_scenario;
        self.options = s.options;
        self.choice_result = s.choice_result;
        self.pending_b = s.pending_b;
        self.last_kind = s.last_kind;
        self.steps = s.steps;
        self.rec_kind = s.rec_kind;
        self.rec_tok_hit = s.rec_tok_hit;
        self.rec_act_hit = s.rec_act_hit;
        self.rec_toks = s.rec_toks;
        self.pending_sprite = s.pending_sprite;
        self.rec_var = s.rec_var;
        self.rec_fun = s.rec_fun;
        self.condition_result = s.condition_result;
        self.choice_pc = s.choice_pc;
    }
}

impl<'a> Interp<'a> {
    pub fn new(p: &'a Program, global: &'a mut GlobalState) -> Interp<'a> {
        let start = p.index.get(&p.entry).copied().unwrap_or(0);
        Interp {
            p,
            global,
            pc: start,
            tok: None,
            ints: Vec::new(),
            flts: Vec::new(),
            speaker: None,
            pending_voice: None,
            voice_scenario: 0,
            options: Vec::new(),
            choice_result: 1,
            pending_b: None,
            last_kind: String::new(),
            steps: 0,
            rec_kind: 0,
            rec_tok_hit: false,
            rec_act_hit: false,
            rec_toks: Vec::new(),
            pending_sprite: None,
            rec_var: None,
            rec_fun: None,
            condition_result: None,
            choice_pc: None,
        }
    }

    /// Values are copied into a save while the interpreter still owns the
    /// mutable borrow of the shared global table.
    pub fn global_snapshot(&self) -> Vec<i32> {
        self.global.snapshot()
    }

    fn goto_off(&mut self, target: u32) -> bool {
        if let Some(k) = self.p.index.get(&target) {
            self.pc = *k;
            true
        } else {
            // Land between ops: advance to the next op after the target.
            let mut best: Option<usize> = None;
            for (k, o) in self.p.ops.iter().enumerate() {
                if op_off(o) >= target {
                    best = Some(k);
                    break;
                }
            }
            match best {
                Some(k) => {
                    self.pc = k;
                    true
                }
                None => false,
            }
        }
    }

    /// Run until an event needs frontend attention.
    pub fn next_event(&mut self) -> Event {
        loop {
            if self.pc >= self.p.ops.len() {
                return Event::End;
            }
            self.steps += 1;
            if self.steps > 20_000_000 {
                return Event::End;
            }
            let op = self.p.ops[self.pc].clone();
            self.pc += 1;
            match op {
                Op::Rec { kind, .. } => {
                    // Finalize the record we just left: a bare `(char, variant,
                    // series*1000)` record followed by a `K16 ACT154/155` record
                    // is a character sprite load.
                    let sprite_ev = self.finish_record();
                    self.ints.clear();
                    self.flts.clear();
                    // Tokens are per-record: a record without a `3, N` op must not
                    // reuse the previous record's token (e.g. K23 `4, 144` = clear).
                    self.tok = None;
                    self.rec_kind = kind;
                    self.rec_tok_hit = false;
                    self.rec_act_hit = false;
                    self.rec_toks.clear();
                    if let Some(ev) = sprite_ev {
                        return ev;
                    }
                }
                Op::Tok { n, .. } => {
                    self.tok = Some(n);
                    self.rec_tok_hit = true;
                    self.rec_toks.push(n);
                }
                Op::Int { v, .. } => self.ints.push(v),
                Op::Flt { v, .. } => self.flts.push(v),
                Op::Goto { target, .. } => {
                    if !self.goto_off(target) {
                        return Event::End;
                    }
                }
                Op::Branch { a, b, .. } => {
                    if let Some(condition) = self.condition_result.take() {
                        // Global conditions in the shipped scripts are emitted
                        // as `Var i; Fun 0; Int value` immediately before this
                        // branch.  True follows the first block; false jumps
                        // to the alternate target.  It is separate from a
                        // player's choice, so loading a save cannot make a
                        // first-time event take the wrong route.
                        if !condition && self.goto_off(a) {
                            // alternate block
                        } else if !condition {
                            return Event::End;
                        }
                        self.pending_b = None;
                        continue;
                    }
                    self.pending_b = Some(b);
                    if self.choice_result >= 2 && self.goto_off(a) {
                        // option 2 continues at A
                    } else if self.choice_result >= 2 {
                        return Event::End;
                    }
                    // option 1 (or state branch): fall through to the case body
                }
                Op::CaseEnd { .. } => {
                    // Merge point: option 1's content band jumps to B here to skip
                    // option 2's band. Option 2 already continued at A, so this is a
                    // no-op for it. Reset the branch state for the next construct.
                    if self.choice_result < 2 {
                        if let Some(b) = self.pending_b.take() {
                            if !self.goto_off(b) {
                                return Event::End;
                            }
                        }
                    } else {
                        self.pending_b = None;
                    }
                    self.choice_result = 1;
                }
                Op::Switch { targets, .. } => {
                    if !targets.is_empty() {
                        let idx = self.choice_result.saturating_sub(1) as usize;
                        let t = targets.get(idx).copied().unwrap_or(targets[0]);
                        if !self.goto_off(t) {
                            return Event::End;
                        }
                    }
                }
                Op::Act { id, .. } => {
                    self.rec_act_hit = true;
                    if let Some(ev) = self.act(id) {
                        return ev;
                    }
                }
                Op::Fun { sub, .. } => {
                    self.rec_fun = Some(sub);
                    // `2, 0` after the option records = present the choice and wait.
                    if sub == 0 {
                        if let Some(options) = self.take_options() {
                            // Remember where this choice began so a save taken
                            // while it is on screen can rewind here on load.
                            self.choice_pc = Some(self.pc.saturating_sub(1));
                            return Event::Choice { options };
                        }
                    }
                }
                Op::Var { sub, .. } => {
                    // Only the indexed entries declared by Global.vrb are
                    // story globals.  Other Var references are local/runtime
                    // values from the original VM and are intentionally left
                    // alone by this persistence layer.
                    if (sub as usize) < self.global.values.len() {
                        self.rec_var = Some(sub);
                    }
                }
                _ => {}
            }
        }
    }

    /// Detect a character sprite-load record: `K30` carrying exactly three ints
    /// `(char_id, variant, series * 1000)` with no token/action, followed by a
    /// `K16` record with `ACT154`/`ACT155`. The file resolved by the frontend is
    /// `{prefix}{series:03}{variant:03}.tga` inside `char.pak`.
    fn finish_record(&mut self) -> Option<Event> {
        // Commit/evaluate global records before the record-local integer
        // buffer is cleared by `next_event`.
        if let Some(index) = self.rec_var.take() {
            if self.rec_fun == Some(0) {
                if let Some(value) = self.ints.last().copied() {
                    let current = self.global.get(index);
                    let matched = current == value;
                    if std::env::var_os("WA2_DEBUG_GLOBAL").is_some() {
                        let name = self
                            .global
                            .names
                            .get(index as usize)
                            .map(String::as_str)
                            .unwrap_or("<unknown>");
                        eprintln!(
                            "global: condition {}[{}] {} == {} -> {}",
                            name,
                            index,
                            current,
                            value,
                            if matched { "true" } else { "false" }
                        );
                    }
                    self.condition_result = Some(matched);
                }
            } else if self.rec_fun.is_none() {
                if let Some(value) = self.ints.last().copied() {
                    let old = self.global.get(index);
                    self.global.set(index, value);
                    if std::env::var_os("WA2_DEBUG_GLOBAL").is_some() && old != value {
                        let name = self
                            .global
                            .names
                            .get(index as usize)
                            .map(String::as_str)
                            .unwrap_or("<unknown>");
                        eprintln!("global: set {}[{}] {} -> {}", name, index, old, value);
                    }
                }
            }
        }
        self.rec_fun = None;
        // A CG parked in a K30 record and picked up by the ACT227 that follows
        // it: `(mode, cg_id, variant, frames, 0, 0, h, n)`.
        //
        // ACT148 carries the same shape *inside* its own record and shows the
        // picture itself, so those records have `rec_act_hit` set and never
        // reach here. ACT227 keeps the payload in the record before it, and
        // that payload used to be dropped with every other "uninteresting"
        // record - the screen stayed on the black ACT146 that precedes it.
        if self.rec_kind == 30 && !self.rec_tok_hit && !self.rec_act_hit && self.ints.len() >= 6 {
            let mode = self.ints[0];
            let cg = self.ints[1];
            if (0..=5).contains(&mode) && cg >= 1000 {
                let mut j = self.pc;
                let mut steps = 0;
                let mut used_by_227 = false;
                while j < self.p.ops.len() && steps < 16 {
                    match &self.p.ops[j] {
                        Op::Act { id, .. } => {
                            used_by_227 = *id == 227;
                            break;
                        }
                        Op::Tok { .. } | Op::Goto { .. } | Op::Return { .. } => break,
                        _ => {}
                    }
                    j += 1;
                    steps += 1;
                }
                if used_by_227 {
                    return Some(Event::ScrollCg {
                        id: cg as i64,
                        variant: self.ints[2].max(0) as u32,
                        frames: self.ints[3].max(0) as u32,
                        pages: self.ints.get(7).copied().unwrap_or(0).max(0) as u32,
                    });
                }
            }
        }
        if self.rec_kind != 30 || self.rec_tok_hit || self.rec_act_hit || self.ints.len() != 3 {
            return None;
        }
        let cid = self.ints[0];
        let variant = self.ints[1];
        let flag = self.ints[2];
        if !((0..40).contains(&cid)
            && (0..1000).contains(&variant)
            && flag % 1000 == 0
            && (1000..=30000).contains(&flag))
        {
            return None;
        }
        let mut j = self.pc;
        let mut steps = 0;
        let mut ok = false;
        while j < self.p.ops.len() && steps < 10 {
            match &self.p.ops[j] {
                Op::Act { id, .. } => match *id {
                    154 | 155 => {
                        ok = true;
                        break;
                    }
                    131 | 132 | 144 => break,
                    _ => {}
                },
                Op::Tok { .. } => break,
                _ => {}
            }
            j += 1;
            steps += 1;
        }
        if !ok {
            return None;
        }
        // Hold it: the K16 record that follows carries the screen slot, and the
        // sprite must be placed there (slots move around during a scene).
        self.pending_sprite = Some(((cid as u32), (flag / 1000) as u32, variant as u32));
        None
    }

    fn act(&mut self, id: u32) -> Option<Event> {
        let tok = self.tok;
        match id {
            131 => {
                // display the current token as dialogue/narration
                let n = tok?;
                let text = self.p.token(n)?.to_string();
                let speaker = self.speaker.as_ref().map(|(name, _)| name.clone());
                let voice = self.pending_voice.take();
                self.speaker = None;
                Some(Event::Text { text, speaker, voice })
            }
            172 => {
                // colour filter (sepia / nega / none), never a background image
                let name = tok.and_then(|n| self.p.token(n)).unwrap_or("").to_string();
                Some(Event::Tint { name })
            }
            193 => {
                // play movie
                let name = tok.and_then(|n| self.p.token(n)).unwrap_or("").to_string();
                if name.is_empty() {
                    return None;
                }
                Some(Event::Movie { name })
            }
            144 => {
                // speaker name plate: with a name token sets it, without one it
                // clears (name plate hidden for the following narration).
                match tok {
                    Some(n) => {
                        if let Some(name) = self.p.token(n) {
                            if !name.is_empty() {
                                self.speaker = Some((name.to_string(), n));
                                self.pending_voice = None;
                            }
                        }
                    }
                    None => {
                        self.speaker = None;
                        self.pending_voice = None;
                    }
                }
                None
            }
            137 => {
                // Select the scenario's voice bank, also used by sub-scripts.
                if self.ints.len() >= 2 && self.ints[1] >= 0 {
                    self.voice_scenario = self.ints[1] as u32;
                }
                None
            }
            138 => {
                // character focus for the current line (dim the others) +
                // voice parameters for the following display
                if self.ints.len() >= 5 {
                    let cid = self.ints[self.ints.len() - 5];
                    let line = *self.ints.last().unwrap();
                    self.pending_voice = if cid >= 0 && line >= 0 {
                        Some(VoiceCue { scenario: self.voice_scenario, line: line as u32, char_id: cid as u32 })
                    } else {
                        None
                    };
                    if (0..40).contains(&cid) {
                        return Some(Event::SpriteFocus { char_id: cid as u32 });
                    }
                }
                None
            }
            208 => {
                // choice option text
                if let Some(n) = tok {
                    if let Some(t) = self.p.token(n) {
                        self.options.push(t.to_string());
                    }
                }
                None
            }
            146 | 147 => {
                // Scene background: (mode, id, variant, frames, 0, 0, 0, w, h).
                // 146 and 147 differ only in transition style.
                if self.ints.len() >= 4 {
                    // K30 and the legacy K16 wrappers both carry the same
                    // `(mode, id, variant, frames, ...)` layout. K23 is the
                    // structural bootstrap record and does not carry the
                    // executable's effect mode. Dropping K16's first value
                    // made `(2001,0,60,...)` resolve to missing `b000600` and
                    // lose its f0 effect plate.
                    let mode = if matches!(self.rec_kind, 16 | 30) { self.ints[0] } else { 0 };
                    let id = self.ints[1] as i64;
                    let variant = self.ints[2].max(0) as u32;
                    let frames = self.ints[3].max(0) as u32;
                    return Some(Event::BgScene { id, variant, frames, mode });
                }
                None
            }
            148 => {
                // Event CG. The exe loads `v{id * 10 + variant:06}.tga`
                // (mode 1, `v%06d.tga`) and puts it up instead of the
                // standing characters. 1012_030's bed shot is id 10170
                // variant 0 → v101700.tga, which lives in IC/grp.pak.
                if self.ints.len() >= 4 && self.ints[1] > 0 {
                    let id = self.ints[1] as i64;
                    let variant = self.ints[2].max(0) as u32;
                    let frames = self.ints[3].max(0) as u32;
                    return Some(Event::EventCg { id, variant, frames });
                }
                None
            }
            152 => {
                // White flash. Args are (subtype, frames, r, g, b). The
                // original snaps to the colour, then lerps back to neutral
                // over `frames`. 1012_030's climax is two 15-frame blinks
                // and one 60-frame blink, all (255, 255, 255).
                if self.ints.len() >= 5 {
                    let frames = self.ints[1].max(0) as u32;
                    let ch = |i: usize| self.ints[i].clamp(0, 255) as u8;
                    return Some(Event::Flash {
                        frames,
                        r: ch(2),
                        g: ch(3),
                        b: ch(4),
                    });
                }
                None
            }
            227 => {
                // The scroll/effects CG. Most calls repeat the picture record
                // inline `(mode, cg_id, variant, frames, ...)`; the ones whose
                // payload sits in the preceding K30 record are raised by
                // `finish_record` instead, and `mode 0` is a clear we leave
                // alone rather than blanking a picture the script did not ask
                // to lose.
                if self.ints.len() >= 4 && self.ints[1] >= 1000 {
                    return Some(Event::ScrollCg {
                        id: self.ints[1] as i64,
                        variant: self.ints[2].max(0) as u32,
                        frames: self.ints[3].max(0) as u32,
                        pages: self.ints.get(7).copied().unwrap_or(0).max(0) as u32,
                    });
                }
                None
            }
            201 => {
                // (cg_id, layer, visible). Layer N is `v{id*10+layer}`.
                // The scripts turn one layer on and the other off; the one
                // turned on replaces the picture. Turning a layer off leaves
                // whatever is already showing.
                if self.ints.len() >= 3 && self.ints[0] > 0 && self.ints[2] != 0 {
                    return Some(Event::EventCg {
                        id: self.ints[0] as i64,
                        variant: self.ints[1].max(0) as u32,
                        frames: 0,
                    });
                }
                None
            }
            176 => {
                // Slot-based overlay graphic: (slot, ?, 2, 256, flag) with
                // tokens [name, "", dir]. Items (a guitar), effect plates and
                // the layers of a composite CG come through here - it is NOT a
                // background change.
                if let Some(n) = self.rec_toks.first().and_then(|t| self.p.token(*t)) {
                    let slot = self.ints.first().copied().unwrap_or(0).max(0) as u32;
                    return Some(Event::Overlay { name: n.to_string(), slot });
                }
                None
            }
            154 | 155 => {
                // Sprite on screen; the K16 record's first value is the slot.
                // 155 is the entrance `(slot, 0, 256, 128)`, 154 the in-place
                // expression change `(slot, 0, 0, frames, 256, 128)`. Both
                // leave the character standing in that slot.
                if let Some((char_id, series, variant)) = self.pending_sprite.take() {
                    let slot = self.ints.first().copied().unwrap_or(0).max(0) as u32;
                    let frames = if id == 155 {
                        15
                    } else {
                        self.ints.get(3).copied().unwrap_or(0).max(0) as u32
                    };
                    return Some(Event::Sprite {
                        char_id,
                        series,
                        variant,
                        slot,
                        frames,
                        kind: if id == 155 { SpriteKind::Enter } else { SpriteKind::Change },
                    });
                }
                None
            }
            170 => {
                // Ambient light preset. The corpus carries exactly one int
                // per call and only ever uses {0, 1, 2, 3}.
                let preset = self.ints.first().copied().unwrap_or(0);
                Some(Event::EnvTint { preset })
            }
            171 => {
                // The exe wrapper reads int#1 as the value and int#6 as the
                // channel selector, then calls 0x401880. In the decoded
                // record representation these are the first and second
                // integer fields respectively (absent fields default to 0).
                let value = self.ints.first().copied().unwrap_or(0);
                let channel = self.ints.get(1).copied().unwrap_or(0);
                Some(Event::WindowAlpha { value, channel })
            }
            156 => {
                // Leave. The first int is the character id, not a slot — the
                // same id K30 used when the face was chosen. Scenes dismiss
                // someone with this and keep talking; without it they stand
                // there until a later placement overwrites their slot.
                let char_id = self.ints.first().copied().unwrap_or(-1);
                if char_id < 0 {
                    return None;
                }
                let frames = self.ints.get(2).copied().unwrap_or(0).max(0) as u32;
                Some(Event::SpriteExit { char_id: char_id as u32, frames })
            }
            194 => {
                // Timed pause: (frames, 0, 0) at 60 fps.
                if let Some(f) = self.ints.first() {
                    if *f > 0 {
                        return Some(Event::WaitFrames(*f as u32));
                    }
                }
                None
            }
            158..=169 => Some(Event::Sound { opcode: id as u16, args: self.ints.clone() }),
            132 => Some(Event::Wait),
            1 => {
                // CATCH / CATCH2 / CATCH3 marker
                if let Some(t) = tok.and_then(|n| self.p.token(n)) {
                    if t.starts_with("CATCH") {
                        self.last_kind = t.to_string();
                    }
                }
                None
            }
            0 => {
                // jump to the target script id
                if let Some(t) = tok.and_then(|n| self.p.token(n)) {
                    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '_') {
                        return Some(Event::Jump {
                            target: t.to_string(),
                            kind: self.last_kind.clone(),
                        });
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Present the pending choice options and take the player's answer.
    ///
    /// The option text is *cloned*, not taken: a save made while the choice is
    /// on screen must still be able to record the options so a later load can
    /// re-present them. `set_choice_result` clears them once the player picks.
    pub fn take_options(&mut self) -> Option<Vec<String>> {
        if self.options.len() >= 2 {
            Some(self.options.clone())
        } else {
            self.options.clear();
            None
        }
    }

    pub fn set_choice_result(&mut self, r: u32) {
        self.choice_result = r.max(1);
        // The choice has been answered: drop the pending re-entry point and the
        // now-consumed option text so the next ACT208 block starts clean.
        self.choice_pc = None;
        self.options.clear();
    }
}

#[cfg(test)]
mod global_vm_tests {
    use super::*;
    use crate::vm::global::GlobalState;

    fn globals(n: usize) -> GlobalState {
        GlobalState {
            names: (0..n).map(|i| format!("g{i}")).collect(),
            defaults: vec![0; n],
            values: vec![0; n],
        }
    }

    fn program(ops: Vec<Op>) -> Program {
        let mut index = HashMap::new();
        for (i, op) in ops.iter().enumerate() {
            index.insert(op_off(op), i);
        }
        Program { ops, index, labels: Vec::new(), tokens: Vec::new(), entry: 0 }
    }

    #[test]
    fn global_assignment_survives_the_record_boundary() {
        let p = program(vec![
            Op::Rec { off: 0, kind: 30 },
            Op::Var { off: 8, sub: 2 },
            Op::Int { off: 16, v: 17 },
            Op::Rec { off: 28, kind: 0 },
            Op::Return { off: 36 },
        ]);
        let mut g = globals(3);
        let mut it = Interp::new(&p, &mut g);
        assert!(matches!(it.next_event(), Event::End));
        assert_eq!(it.global.get(2), 17);
    }

    #[test]
    fn global_equality_controls_a_non_choice_branch() {
        let p = program(vec![
            Op::Rec { off: 0, kind: 30 },
            Op::Var { off: 8, sub: 1 },
            Op::Fun { off: 16, sub: 0 },
            Op::Int { off: 24, v: 7 },
            Op::Rec { off: 36, kind: 27 },
            Op::Rec { off: 44, kind: 0 },
            Op::Branch { off: 52, a: 100, b: 0 },
            Op::Tok { off: 68, n: 0 },
            Op::Act { off: 76, id: 131 },
            Op::Tok { off: 100, n: 1 },
            Op::Act { off: 108, id: 131 },
        ]);
        let mut p = p;
        p.tokens = vec!["true".into(), "false".into()];
        let mut g = globals(2);
        g.set(1, 7);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::Text { text, .. } => assert_eq!(text, "true"),
            other => panic!("expected true branch text, got {other:?}"),
        }
        // A true condition falls through to the first return. With a false
        // value the branch target is the second return; both paths terminate,
        // but this asserts that the condition was actually consumed rather
        // than the player's default choice being consulted.
        g.set(1, 6);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::Text { text, .. } => assert_eq!(text, "false"),
            other => panic!("expected false branch text, got {other:?}"),
        }
    }

    /// ACT170 is the ambient light preset (0 none / 1 evening / 2 night /
    /// 3 indoor), not a message-window switch: the record's first int is
    /// carried straight through.
    #[test]
    fn act170_emits_the_ambient_preset() {
        for preset in [0, 1, 2, 3] {
            let p = program(vec![
                Op::Rec { off: 0, kind: 0 },
                Op::Int { off: 8, v: preset },
                Op::Act { off: 16, id: 170 },
                Op::Rec { off: 24, kind: 0 },
                Op::Return { off: 32 },
            ]);
            let mut g = globals(1);
            let mut it = Interp::new(&p, &mut g);
            match it.next_event() {
                Event::EnvTint { preset: got } => assert_eq!(got, preset),
                other => panic!("expected EnvTint({preset}), got {other:?}"),
            }
        }
    }

    #[test]
    fn act171_preserves_value_and_channel() {
        let p = program(vec![
            Op::Rec { off: 0, kind: 30 },
            Op::Int { off: 8, v: 255 },
            Op::Int { off: 20, v: 0 },
            Op::Act { off: 32, id: 171 },
            Op::Rec { off: 40, kind: 0 },
            Op::Return { off: 48 },
        ]);
        let mut g = globals(1);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::WindowAlpha { value, channel } => assert_eq!((value, channel), (255, 0)),
            other => panic!("expected WindowAlpha, got {other:?}"),
        }
    }

    #[test]
    fn sprite_enter_change_and_exit_preserve_slot_and_frames() {
        let p = program(vec![
            // K30 face selection followed by K16 ACT155 entrance.
            Op::Rec { off: 0, kind: 30 },
            Op::Int { off: 8, v: 10 },
            Op::Int { off: 20, v: 111 },
            Op::Int { off: 32, v: 1000 },
            Op::Rec { off: 44, kind: 16 },
            Op::Int { off: 52, v: 1 },
            Op::Int { off: 64, v: 0 },
            Op::Int { off: 76, v: 256 },
            Op::Int { off: 88, v: 128 },
            Op::Act { off: 100, id: 155 },
            // Same character, new face, K16 ACT154 in the same slot.
            Op::Rec { off: 108, kind: 30 },
            Op::Int { off: 116, v: 10 },
            Op::Int { off: 128, v: 103 },
            Op::Int { off: 140, v: 1000 },
            Op::Rec { off: 152, kind: 16 },
            Op::Int { off: 160, v: 1 },
            Op::Int { off: 172, v: 0 },
            Op::Int { off: 184, v: 0 },
            Op::Int { off: 196, v: 12 },
            Op::Int { off: 208, v: 256 },
            Op::Int { off: 220, v: 128 },
            Op::Act { off: 232, id: 154 },
            // ACT156 removes the character by ID, not by slot.
            Op::Rec { off: 240, kind: 16 },
            Op::Int { off: 248, v: 10 },
            Op::Int { off: 260, v: 0 },
            Op::Int { off: 272, v: 20 },
            Op::Act { off: 284, id: 156 },
            Op::Rec { off: 292, kind: 0 },
            Op::Return { off: 300 },
        ]);
        let mut g = globals(1);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::Sprite { char_id, variant, series, slot, frames, kind } => {
                assert_eq!((char_id, variant, series, slot, frames), (10, 111, 1, 1, 15));
                assert_eq!(kind, SpriteKind::Enter);
            }
            other => panic!("expected sprite entrance, got {other:?}"),
        }
        match it.next_event() {
            Event::Sprite { char_id, variant, series, slot, frames, kind } => {
                assert_eq!((char_id, variant, series, slot, frames), (10, 103, 1, 1, 12));
                assert_eq!(kind, SpriteKind::Change);
            }
            other => panic!("expected sprite change, got {other:?}"),
        }
        match it.next_event() {
            Event::SpriteExit { char_id, frames } => assert_eq!((char_id, frames), (10, 20)),
            other => panic!("expected sprite exit, got {other:?}"),
        }
    }

    #[test]
    fn background_mode_is_forwarded_only_from_k30() {
        for kind in [30, 23] {
            let p = program(vec![
                Op::Rec { off: 0, kind },
                Op::Int { off: 8, v: 0xA1 },
                Op::Int { off: 20, v: 2025 },
                Op::Int { off: 32, v: 1 },
                Op::Int { off: 44, v: 60 },
                Op::Act { off: 56, id: 146 },
                Op::Rec { off: 64, kind: 0 },
                Op::Return { off: 72 },
            ]);
            let mut g = globals(1);
            let mut it = Interp::new(&p, &mut g);
            match it.next_event() {
                Event::BgScene { id, variant, frames, mode } => {
                    assert_eq!((id, variant, frames), (2025, 1, 60));
                    assert_eq!(mode, if kind == 30 { 0xA1 } else { 0 });
                }
                other => panic!("expected BgScene, got {other:?}"),
            }
        }
    }

    #[test]
    fn legacy_k16_background_forwards_effect_mode() {
        let p = program(vec![
            Op::Rec { off: 0, kind: 16 },
            Op::Int { off: 8, v: 2001 },
            Op::Int { off: 20, v: 0 },
            Op::Int { off: 32, v: 60 },
            Op::Int { off: 44, v: 0 },
            Op::Int { off: 56, v: 0 },
            Op::Int { off: 68, v: 0 },
            Op::Flt { off: 80, v: 1280.0 },
            Op::Flt { off: 92, v: 720.0 },
            Op::Act { off: 104, id: 146 },
            Op::Rec { off: 112, kind: 0 },
            Op::Return { off: 120 },
        ]);
        let mut g = globals(1);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::BgScene { id, variant, frames, mode } => {
                assert_eq!((id, variant, frames), (0, 60, 0));
                assert_eq!(mode, 2001);
            }
            other => panic!("expected K16 BgScene, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_while_a_choice_is_shown_rewinds_to_the_choice() {
        // Two ACT208 records collect the option text, then Fun{sub=0}
        // presents the choice and waits. `next_event` has already advanced pc
        // past the Fun by the time it returns `Choice`, so a save taken on the
        // menu must rewind pc back to the Fun and keep the option text, or the
        // reload would resume *after* the choice with the default answer.
        let mut p = program(vec![
            Op::Rec { off: 0, kind: 30 },
            Op::Tok { off: 8, n: 0 },
            Op::Act { off: 16, id: 208 },
            Op::Rec { off: 24, kind: 30 },
            Op::Tok { off: 32, n: 1 },
            Op::Act { off: 40, id: 208 },
            Op::Rec { off: 48, kind: 30 },
            Op::Fun { off: 56, sub: 0 },
            Op::Rec { off: 64, kind: 0 },
            Op::Return { off: 72 },
        ]);
        p.tokens = vec!["再去见她".into(), "不再见面".into()];
        let mut g = globals(1);
        let mut it = Interp::new(&p, &mut g);
        match it.next_event() {
            Event::Choice { options } => assert_eq!(options, vec!["再去见她", "不再见面"]),
            other => panic!("expected Choice, got {other:?}"),
        }
        // The save snapshot rewinds pc to the Fun and preserves the options.
        let snap = it.snapshot();
        assert_eq!(snap.options, vec!["再去见她", "不再见面"]);
        assert_eq!(snap.choice_pc, None, "rewind is already folded into pc");
        // Reloading from that snapshot re-presents the same choice.
        let mut restored = Interp::new(&p, &mut g);
        restored.restore(snap);
        match restored.next_event() {
            Event::Choice { options } => assert_eq!(options, vec!["再去见她", "不再见面"]),
            other => panic!("expected re-presented Choice after load, got {other:?}"),
        }
        // Answering clears the re-entry state so the next choice starts clean.
        restored.set_choice_result(2);
        assert_eq!(restored.choice_result, 2);
        assert!(restored.options.is_empty());
    }
}
