//! Audio mixer: SDL2 audio device + software mixing of multiple PCM streams.
//!
//! Stream decoding (Ogg Vorbis from LAC, WMA from ASF) is delegated to an
//! `ffmpeg` subprocess that writes raw interleaved s16le PCM to a pipe; this
//! module pulls that PCM on a background thread, keeps a ring buffer per
//! stream, applies per-stream volume/pan, mixes to a master buffer, and hands
//! it to SDL_QueueAudio. The SDL audio callback is not used (we push from the
//! mixer thread instead) which keeps mixing simple and deterministic.

use std::collections::VecDeque;
use std::io::Read;
use std::os::fd::AsRawFd;
unsafe extern "C" { fn fcntl(fd: i32, cmd: i32, ...) -> i32; }
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// `fcntl(2)` command and flag values, spelled out because this file talks to
/// libc directly and they differ between the platforms the engine runs on.
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0x800;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;
/// Frames rendered per mix call. 1024 stereo frames is 21.33 ms at 48 kHz and
/// matches the device buffer requested in `sdl_open_audio`.
pub const BLOCK_FRAMES: usize = 1024;
/// Bytes of interleaved s16le produced by one block.
pub const BLOCK_BYTES: u32 = (BLOCK_FRAMES * CHANNELS as usize * 2) as u32;

/// Samples a single stream may hold ahead of the mixer. 8192 s16 samples is
/// 4096 stereo frames, about 85 ms - the same slack the device queue keeps.
const PCM_TARGET: usize = 8192;

/// Mixer categories. The option screen's four volume sliders map onto these:
/// `all` scales everything, the others scale one kind of sound.
pub const CAT_ALL: usize = 0;
pub const CAT_BGM: usize = 1;
pub const CAT_SE: usize = 2;
pub const CAT_VOICE: usize = 3;
pub const CAT_COUNT: usize = 4;

use std::sync::atomic::{AtomicU64, Ordering};
static PULLED: AtomicU64 = AtomicU64::new(0);
static QUEUED: AtomicU64 = AtomicU64::new(0);

/// Total PCM samples pulled from decoder subprocesses (diagnostic).
pub fn pulled_samples() -> u64 { PULLED.load(Ordering::Relaxed) }
/// Total bytes pushed to the SDL audio device (diagnostic).
pub fn queued_bytes() -> u64 { QUEUED.load(Ordering::Relaxed) }

/// One decoded audio stream (BGM, SE, voice, ...).
pub struct Stream {
    /// Source path, for diagnostics only.
    label: String,
    /// Which volume slider scales this stream.
    cat: usize,
    carry: Vec<u8>,
    child: Option<Child>,
    /// Fully decoded PCM held in memory instead of piped from a decoder.
    ///
    /// Short UI sounds are played far more often than any other kind of audio,
    /// and spawning `ffmpeg` for one costs tens of milliseconds - on a cold
    /// cache hundreds - which is long enough to be seen as a dropped frame and
    /// heard as a hole in the music. When this is set the worker feeds `pcm`
    /// straight from here and there is no child process at all.
    mem: Option<Arc<Vec<i16>>>,
    mem_pos: usize,
    /// raw s16le frames not yet mixed
    pcm: VecDeque<i16>,
    pub volume: f32,
    pub looping: bool,
    eos: bool,
    ramp: Option<(f32, f32, std::time::Instant, std::time::Duration, bool)>,
}

impl Stream {
    /// Spawn `ffmpeg -i <in> -f s16le -ar 48000 -ac 2 -` and stream-decode it.
    pub fn spawn_ffmpeg(input: &str, looping: bool) -> Option<Stream> {
        let mut command = Command::new("ffmpeg");
        command.args(["-hide_banner", "-loglevel", "error"]);
        if looping { command.args(["-stream_loop", "-1"]); }
        let child = command.args(["-i", input, "-f", "s16le", "-ar", "48000", "-ac", "2", "-"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        // No decoder may hold the mixer lock while waiting.
        //
        // The flag value is per-platform: Linux O_NONBLOCK is 0x800, macOS
        // (and every BSD) is 0x0004. Using the Linux constant here meant
        // F_SETFL silently set a bit the kernel ignores, the pipe stayed
        // blocking, and one slow decoder stalled the whole mixer thread -
        // including every other stream it was supposed to be feeding.
        let fd = child.stdout.as_ref()?.as_raw_fd();
        unsafe {
            let flags = fcntl(fd, F_GETFL);
            if flags >= 0 {
                fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            }
        }
        Some(Stream { label: input.to_string(), cat: CAT_ALL, carry: Vec::new(),
            child: Some(child), mem: None, mem_pos: 0,
            pcm: VecDeque::new(), volume: 1.0, looping, eos: false,
            ramp: None })
    }

    /// A stream backed by an already decoded buffer.
    fn from_pcm(pcm: Arc<Vec<i16>>, looping: bool) -> Stream {
        Stream {
            label: String::from("<pcm>"),
            cat: CAT_ALL,
            carry: Vec::new(),
            child: None,
            mem: Some(pcm),
            mem_pos: 0,
            pcm: VecDeque::new(),
            volume: 1.0,
            looping,
            eos: false,
            ramp: None,
        }
    }
}

/// Decode a whole file to 48 kHz stereo s16le in memory.
///
/// Only meant for files whose duration is known to be short (UI sounds); the
/// caller holds the buffer for as long as the sound may be replayed, so this
/// must not be used for BGM or voice.
pub fn decode_pcm(path: &std::path::Path) -> Option<Arc<Vec<i16>>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "s16le", "-ar", "48000", "-ac", "2", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.len() < 2 {
        return None;
    }
    let mut pcm = Vec::with_capacity(out.stdout.len() / 2);
    for b in out.stdout.chunks_exact(2) {
        pcm.push(i16::from_le_bytes([b[0], b[1]]));
    }
    Some(Arc::new(pcm))
}

/// The mixer owns the SDL audio device and all active streams.
pub struct Mixer {
    inner: Arc<Mutex<MixerInner>>,
    worker: Option<thread::JoinHandle<()>>,
}

struct MixerInner {
    streams: Vec<Option<Stream>>,
    /// gains[CAT_ALL] * gains[category] scales every stream.
    gains: [f32; CAT_COUNT],
    quit: bool,
}

impl Mixer {
    /// Open the SDL audio device and start the mixing thread.
    /// Returns None if SDL audio could not be initialised.
    pub fn new(sdl: &crate::sdl2::Sdl) -> Option<Mixer> {
        let dev = crate::sdl2::sdl_open_audio(SAMPLE_RATE, CHANNELS);
        if dev == 0 {
            return None;
        }
        let inner = Arc::new(Mutex::new(MixerInner {
            streams: Vec::new(),
            gains: [1.0; CAT_COUNT],
            quit: false,
        }));
        let inner2 = Arc::clone(&inner);
        let worker = thread::spawn(move || {

            while !inner2.lock().unwrap().quit {
                // pull any available PCM from children
                {
                    let mut g = inner2.lock().unwrap();
                    for s in g.streams.iter_mut() {
                        if let Some(st) = s {
                            if st.eos || st.pcm.len() >= PCM_TARGET { continue; }
                            // In-memory source: refill from the buffer, no pipe.
                            if let Some(src) = st.mem.clone() {
                                while st.pcm.len() < PCM_TARGET && st.mem_pos < src.len() {
                                    let n = (PCM_TARGET - st.pcm.len()).min(src.len() - st.mem_pos);
                                    st.pcm.extend(src[st.mem_pos..st.mem_pos + n].iter().copied());
                                    st.mem_pos += n;
                                }
                                if st.mem_pos >= src.len() {
                                    if st.looping {
                                        st.mem_pos = 0;
                                    } else {
                                        st.eos = true;
                                    }
                                }
                                continue;
                            }
                            let mut buf = [0u8; 4096];
                            // Refill the stream's own queue rather than
                            // reading one buffer per tick: at 2 ms per tick and
                            // 21 ms of PCM per read, a stream whose queue had
                            // run dry could not catch up while the device was
                            // consuming it, and the output ran at a lower depth
                            // than the target - audible as dropouts on macOS,
                            // where an underrun makes the driver repeat the
                            // last buffer instead of going silent.
                            let mut taken = 0usize;
                            loop {
                                if st.pcm.len() >= PCM_TARGET || taken >= 32 * 1024 {
                                    break;
                                }
                                let Some(child) = st.child.as_mut() else { break };
                                let Some(out) = child.stdout.as_mut() else { break };
                                match out.read(&mut buf) {
                                    Ok(0) => {
                                        st.eos = true;
                                        // Decoder exited on its own: reap it, or every
                                        // finished SE/voice line stays a zombie.
                                        if let Some(c) = st.child.as_mut() {
                                            if matches!(c.try_wait(), Ok(Some(_))) {
                                                st.child = None;
                                            }
                                        }
                                        break;
                                    }
                                    Ok(n) => {
                                        taken += n;
                                        st.carry.extend_from_slice(&buf[..n]);
                                        let usable = st.carry.len() - (st.carry.len() % 2);
                                        for ch in st.carry[..usable].chunks_exact(2) {
                                            st.pcm.push_back(i16::from_le_bytes([ch[0], ch[1]]));
                                        }
                                        PULLED.fetch_add((usable / 2) as u64, Ordering::Relaxed);
                                        st.carry.drain(..usable);
                                    }
                                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                    Err(_) => {
                                        st.eos = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // A decoder can exit a moment after EOF; collect those too so
                    // finished streams never linger as zombies.
                    for st in g.streams.iter_mut().flatten() {
                        if st.eos {
                            if let Some(c) = st.child.as_mut() {
                                if matches!(c.try_wait(), Ok(Some(_))) {
                                    st.child = None;
                                }
                            }
                        }
                    }
                }
                // mix & push
                static LAST_DBG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                if std::env::var_os("WA2_DEBUG_AUDIO").is_some() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let prev = LAST_DBG.load(Ordering::Relaxed);
                    if now.saturating_sub(prev) >= 1000 {
                        LAST_DBG.store(now, Ordering::Relaxed);
                        let g = inner2.lock().unwrap();
                        let gains = g.gains;
                        let names: Vec<String> = g
                            .streams
                            .iter()
                            .filter_map(|s| s.as_ref())
                            .filter(|st| !st.pcm.is_empty() && st.volume > 0.001)
                            .map(|st| {
                                let p = std::path::Path::new(&st.label);
                                let f = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                                let eff = st.volume * gains[0] * gains[st.cat];
                                if eff < 0.99 { format!("{}@{:.2}", f, eff) } else { f }
                            })
                            .collect();
                        let slots = g.streams.len();
                        drop(g);
                        let depth_ms = crate::sdl2::sdl_queued_audio_bytes(dev)
                            .map(|b| b as f32 / (SAMPLE_RATE as f32 * CHANNELS as f32 * 2.0) * 1000.0);
                        eprintln!(
                            "  audio: n={} slots={} depth={:?} [{}]",
                            names.len(),
                            slots,
                            depth_ms.map(|d| format!("{d:.0}ms")),
                            names.join(" + ")
                        );
                    }
                }
                // Keep the device topped up to a fixed depth.
                //
                // This used to push exactly one block per 21.33 ms sleep. That
                // is open-loop pacing: the sleep always overshoots a little, the
                // queue erodes, and on a machine whose cores are busy compositing
                // a 2560x1440 frame the scheduler can delay this thread for tens
                // of milliseconds - at which point the device runs dry and the
                // music clicks. Ask SDL how much it still has and refill only
                // what is missing; the queue depth then absorbs scheduling
                // jitter instead of turning it into dropouts.
                const TARGET_BYTES: u32 = BLOCK_BYTES * 4; // ~85 ms of slack
                let mut pushed = 0u32;
                loop {
                    if pushed >= 8 {
                        break;
                    }
                    match crate::sdl2::sdl_queued_audio_bytes(dev) {
                        Some(q) if q + BLOCK_BYTES <= TARGET_BYTES => {}
                        // Full enough, or SDL cannot tell us: stop refilling.
                        Some(_) => break,
                        // Pre-2.0.4 SDL has no queue query; fall back to the old
                        // blind one-block-per-tick pacing.
                        None => {
                            if pushed > 0 {
                                break;
                            }
                        }
                    }
                    let block = mix_once(&inner2);
                    QUEUED.fetch_add(block.len() as u64, Ordering::Relaxed);
                    crate::sdl2::sdl_queue_audio(dev, &block);
                    pushed += 1;
                    if crate::sdl2::sdl_queued_audio_bytes(dev).is_none() {
                        break;
                    }
                }
                // Short naps: the queue is deep, so a few milliseconds of slack
                // costs nothing and keeps refills responsive after a stall.
                thread::sleep(std::time::Duration::from_millis(if pushed > 0 { 2 } else { 5 }));
            }
        });
        crate::sdl2::sdl_pause_audio(dev, false);
        Some(Mixer { inner, worker: Some(worker) })
    }

    /// Allocate a stream slot and start an ffmpeg decode into it.
    pub fn play(&self, input: &str, volume: f32, looping: bool) -> usize {
        self.play_cat(input, volume, looping, CAT_ALL)
    }

    /// Like [`Mixer::play`] but tags the stream with a volume category.
    pub fn play_cat(&self, input: &str, volume: f32, looping: bool, cat: usize) -> usize {
        let mut g = self.inner.lock().unwrap();
        let mut s = match Stream::spawn_ffmpeg(input, looping) {
            Some(s) => s, None => return usize::MAX,
        };
        s.volume = volume;
        s.looping = looping;
        s.cat = cat.min(CAT_COUNT - 1);
        g.streams.push(Some(s));
        g.streams.len() - 1
    }

    /// Play an already decoded buffer.
    ///
    /// This is what UI sounds use: no process to spawn, no pipe to refill, so
    /// the call is a mutex acquisition and a push - small enough to sit in the
    /// frame that handled the key press without costing a frame.
    pub fn play_pcm(&self, pcm: Arc<Vec<i16>>, volume: f32, looping: bool, cat: usize) -> usize {
        let mut s = Stream::from_pcm(pcm, looping);
        s.volume = volume;
        s.looping = looping;
        s.cat = cat.min(CAT_COUNT - 1);
        let mut g = self.inner.lock().unwrap();
        // Reuse a finished slot instead of pushing a new one.
        //
        // This is called on every key press, and whoever starts a menu sound
        // never stops it, so a dead slot is free for the taking. Reusing keeps
        // the stream list the size of "sounds playing right now" rather than
        // growing by one per click - 40 clicks in a row left 41 slots behind,
        // and the mix loop walks every one of them on every block.
        let dead = g.streams.iter().position(|s| {
            s.as_ref()
                .map(|st| st.eos && st.pcm.is_empty() && st.child.is_none())
                .unwrap_or(false)
        });
        match dead {
            Some(i) => {
                g.streams[i] = Some(s);
                i
            }
            None => {
                g.streams.push(Some(s));
                g.streams.len() - 1
            }
        }
    }

    /// Set one of the option screen's volume sliders (linear gain, 1.0 = 100%).
    pub fn set_gain(&self, cat: usize, gain: f32) {
        if cat >= CAT_COUNT {
            return;
        }
        self.inner.lock().unwrap().gains[cat] = gain.clamp(0.0, 2.0);
    }

    pub fn gain(&self, cat: usize) -> f32 {
        if cat >= CAT_COUNT {
            return 1.0;
        }
        self.inner.lock().unwrap().gains[cat]
    }

    /// Script fades use frames at 60 Hz; nonpositive durations are immediate.
    pub fn fade(&self, idx: usize, target: f32, frames: i32, stop: bool) {
        if frames <= 0 && stop { self.stop(idx); return; }
        let mut g = self.inner.lock().unwrap();
        if let Some(Some(s)) = g.streams.get_mut(idx) {
            if frames <= 0 { s.volume = target; s.ramp = None; }
            else { s.ramp = Some((s.volume, target, std::time::Instant::now(),
                std::time::Duration::from_secs_f64(frames as f64 / 60.0), stop)); }
        }
    }

    pub fn settled(&self, idx: usize) -> bool {
        let g = self.inner.lock().unwrap();
        g.streams.get(idx).and_then(|s| s.as_ref()).map(|s| s.ramp.is_none()).unwrap_or(true)
    }

    pub fn finished(&self, idx: usize) -> bool {
        let g = self.inner.lock().unwrap();
        g.streams.get(idx).and_then(|s| s.as_ref())
            .map(|s| s.eos && s.pcm.is_empty()).unwrap_or(true)
    }

    pub fn stop(&self, idx: usize) {
        let mut g = self.inner.lock().unwrap();
        if idx < g.streams.len() {
            if let Some(mut s) = g.streams[idx].take() {
                if let Some(mut c) = s.child.take() { let _ = c.kill(); let _ = c.wait(); }
            }
        }
    }

    pub fn stop_all(&self) {
        let n = { self.inner.lock().unwrap().streams.len() };
        for i in 0..n { self.stop(i); }
    }

    /// Stop every stream in one category, including streams owned by a modal
    /// screen rather than by the scene interpreter. This is used at the game
    /// boundary so title/music BGM cannot leak into the first game scene.
    pub fn stop_category(&self, cat: usize) {
        let mut g = self.inner.lock().unwrap();
        for stream in g.streams.iter_mut() {
            let matches = stream.as_ref().map(|s| s.cat == cat).unwrap_or(false);
            if !matches {
                continue;
            }
            if let Some(mut s) = stream.take() {
                if let Some(mut child) = s.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.inner.lock().unwrap().quit = true;
        if let Some(h) = self.worker.take() { let _ = h.join(); }
        self.stop_all();
    }
}

/// Mix one block (assume 1024 stereo frames) from all active streams.
fn mix_once(inner: &Arc<Mutex<MixerInner>>) -> Vec<u8> {
    let frames = BLOCK_FRAMES * CHANNELS as usize;
    let mut acc = vec![0i32; frames];
    {
        let mut g = inner.lock().unwrap();
        let g_gains = g.gains;
        for s in g.streams.iter_mut() {
            if let Some(st) = s {
                if let Some((from, to, start, duration, stop)) = st.ramp {
                    let fraction = (start.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0);
                    st.volume = from + (to - from) * fraction;
                    if fraction >= 1.0 {
                        st.ramp = None;
                        if stop {
                            if let Some(mut c) = st.child.take() { let _ = c.kill(); let _ = c.wait(); }
                            st.looping = false;
                            st.eos = true;
                            st.pcm.clear();
                        }
                    }
                }
                let g = g_gains[0] * g_gains[st.cat];
                let vol = (st.volume * g * 65536.0) as i32;
                for i in 0..frames {
                    let v = match st.pcm.pop_front() {
                        Some(x) => x as i32,
                        None => {
                            if st.eos {
                                0
                            } else {
                                break;
                            }
                        }
                    };
                    acc[i] += (v * vol) >> 16;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(frames * 2);
    for v in acc {
        let c = v.clamp(-32768, 32767) as i16;
        out.extend_from_slice(&c.to_le_bytes());
    }
    out
}
