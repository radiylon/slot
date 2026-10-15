use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use libloading::Library;

use super::pacer::Pacer;
use super::ring::Ring;
use super::silence::Silence;
use super::sink::{AudioError, AudioSink};

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const CHANNELS: c_uint = 2;

pub const LATENCY_US: c_uint = 40_000;

pub const BLUETOOTH_LATENCY_US: c_uint = 20_000;

const PERIOD_FRAMES: usize = 512;

const RELEASE_AFTER: Duration = Duration::from_secs(3);

/// Closing and reopening a headphone stream inside a few seconds leaves AirPods silent, and
/// every reopen costs the headphones' wake-up, so a rewind or a pause holds the stream.
const BLUETOOTH_RELEASE_AFTER: Duration = Duration::from_secs(60);

fn release_after(route: Option<&str>) -> Duration {
    match route {
        Some(_) => BLUETOOTH_RELEASE_AFTER,
        None => RELEASE_AFTER,
    }
}

const DEVICES: [&str; 4] = ["plug:default", "default", "plughw:0,0", "hw:0,0"];

const ROUTE_CHECK: Duration = Duration::from_millis(250);

const ROUTE_RETRY: Duration = Duration::from_secs(1);

const DELAY_REPORT: Duration = Duration::from_secs(10);

/// The ALSA device bluetooth.sh routes to, or None for the speaker. The file stays while
/// the headphones drop out, so their silence is never filled by the speaker.
fn route() -> Option<String> {
    let path = std::env::var_os("SLOT_BT_ROUTE").unwrap_or_else(|| "/run/slot-bt-route".into());
    bluetooth_device(&std::fs::read_to_string(path).ok()?)
}

/// One plug in front of BlueALSA converts 32768 Hz to the headphones' 48 kHz. Opening it
/// through "plug:default" would stack a second plug and trip alsa-lib's own assertion.
pub fn bluetooth_device(route: &str) -> Option<String> {
    let mac = route
        .trim()
        .strip_prefix("bluealsa:DEV=")?
        .strip_suffix(",PROFILE=a2dp")?;
    let parts: Vec<&str> = mac.split(':').collect();
    let hex = |p: &&str| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit());
    (parts.len() == 6 && parts.iter().all(hex))
        .then(|| format!("plug:SLAVE='bluealsa:DEV={mac},PROFILE=a2dp'"))
}

type PcmOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type PcmSetParams =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type PcmWritei = unsafe extern "C" fn(*mut c_void, *const c_void, u64) -> i64;
type PcmRecover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type PcmDrop = unsafe extern "C" fn(*mut c_void) -> c_int;
type PcmClose = unsafe extern "C" fn(*mut c_void) -> c_int;
type PcmDelay = unsafe extern "C" fn(*mut c_void, *mut std::ffi::c_long) -> c_int;
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

struct Alsa {
    open: PcmOpen,
    set_params: PcmSetParams,
    writei: PcmWritei,
    recover: PcmRecover,
    drop: PcmDrop,
    close: PcmClose,
    delay: PcmDelay,
    strerror: StrError,
    _lib: Library,
}

impl Alsa {
    fn load() -> Result<Self, AudioError> {
        let lib = unsafe { Library::new("libasound.so.2") }
            .or_else(|_| unsafe { Library::new("libasound.so") })
            .map_err(|e| AudioError::Device(format!("libasound: {e}")))?;
        macro_rules! get {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes())
                    .map_err(|e| AudioError::Device(format!("{}: {e}", $name)))?
            };
        }
        unsafe {
            Ok(Alsa {
                open: get!("snd_pcm_open"),
                set_params: get!("snd_pcm_set_params"),
                writei: get!("snd_pcm_writei"),
                recover: get!("snd_pcm_recover"),
                drop: get!("snd_pcm_drop"),
                close: get!("snd_pcm_close"),
                delay: get!("snd_pcm_delay"),
                strerror: get!("snd_strerror"),
                _lib: lib,
            })
        }
    }

    fn message(&self, err: c_int) -> String {
        let text = unsafe { (self.strerror)(err) };
        match text.is_null() {
            true => format!("error {err}"),
            false => unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned(),
        }
    }

    fn open_pcm(&self, rate: u32, route: Option<&str>) -> Result<*mut c_void, AudioError> {
        let mut last = AudioError::NoDevice;
        let (names, latency): (Vec<&str>, c_uint) = match route {
            Some(device) => (vec![device], bluetooth_latency_us()),
            None => (DEVICES.to_vec(), LATENCY_US),
        };
        for name in names {
            let Ok(cname) = CString::new(name) else {
                continue;
            };
            let mut pcm: *mut c_void = std::ptr::null_mut();
            let err = unsafe { (self.open)(&mut pcm, cname.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
            if err < 0 || pcm.is_null() {
                last = AudioError::Device(format!("{name}: {}", self.message(err)));
                continue;
            }
            let err = unsafe {
                (self.set_params)(
                    pcm,
                    SND_PCM_FORMAT_S16_LE,
                    SND_PCM_ACCESS_RW_INTERLEAVED,
                    CHANNELS,
                    rate,
                    1,
                    latency,
                )
            };
            if err < 0 {
                unsafe { (self.close)(pcm) };
                last = AudioError::Config(format!("{name}: {}", self.message(err)));
                continue;
            }
            eprintln!(
                "slot: audio {name} at {rate} Hz, {} ms buffer",
                latency / 1000
            );
            return Ok(pcm);
        }
        Err(last)
    }
}

pub struct AlsaSink {
    ring: Arc<Ring>,
    device: Option<Device>,
}

struct Device {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl AlsaSink {
    pub fn new() -> Self {
        AlsaSink {
            ring: Arc::new(Ring::new(0)),
            device: None,
        }
    }

    fn close(&mut self) {
        if let Some(d) = self.device.take() {
            d.stop.store(true, Ordering::Relaxed);
            let _ = d.join.join();
        }
    }
}

impl Default for AlsaSink {
    fn default() -> Self {
        AlsaSink::new()
    }
}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        self.close();
    }
}

impl AudioSink for AlsaSink {
    fn open(&mut self, sample_rate: u32) -> Result<(), AudioError> {
        self.close();
        let ring = self.ring.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("slot-audio".into())
            .spawn(move || match play(&ring, sample_rate) {
                Ok(mut device) => {
                    let _ = ready_tx.send(Ok(()));
                    device.run(&ring, &flag);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| AudioError::Device(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.device = Some(Device { stop, join });
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => {
                let _ = join.join();
                Err(AudioError::Device("output thread stopped".into()))
            }
        }
    }

    fn ring(&self) -> Arc<Ring> {
        self.ring.clone()
    }
}

struct Playback {
    alsa: Alsa,
    pcm: Option<*mut c_void>,
    rate: u32,
    failing: bool,
    route: Option<String>,
    route_at: Instant,
    retry_at: Instant,
    reported_at: Instant,
}

/// A whole number of milliseconds from the environment, for tuning headphone latency on
/// the device without a rebuild.
fn tuned_ms(name: &str) -> Option<u32> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|ms| (5..=500).contains(ms))
}

fn bluetooth_latency_us() -> c_uint {
    tuned_ms("SLOT_BT_LATENCY_MS").map_or(BLUETOOTH_LATENCY_US, |ms| ms * 1000)
}

fn reopen_ring(ring: &Ring, sample_rate: u32, route: Option<&str>) {
    match route {
        Some(_) => {
            let frames = tuned_ms("SLOT_BT_RING_MS").map_or_else(
                || super::bluetooth_ring_capacity(sample_rate),
                |ms| sample_rate as usize * ms as usize / 1000,
            );
            ring.reopen_with(sample_rate, frames)
        }
        None => ring.reopen(sample_rate),
    }
}

fn play(ring: &Arc<Ring>, sample_rate: u32) -> Result<Playback, AudioError> {
    let alsa = Alsa::load()?;
    let route = route();
    let pcm = match alsa.open_pcm(sample_rate, route.as_deref()) {
        Ok(pcm) => Some(pcm),
        Err(e) => {
            eprintln!("slot: audio: {e}, retrying");
            None
        }
    };
    reopen_ring(ring, sample_rate, route.as_deref());
    let now = Instant::now();
    Ok(Playback {
        alsa,
        pcm,
        rate: sample_rate,
        failing: pcm.is_none(),
        route,
        route_at: now,
        retry_at: now,
        reported_at: now,
    })
}

impl Playback {
    fn run(&mut self, ring: &Ring, stop: &AtomicBool) {
        let mut buf = vec![0i16; PERIOD_FRAMES * CHANNELS as usize];
        let span = Duration::from_secs_f64(PERIOD_FRAMES as f64 / self.rate as f64);
        let mut silence = Silence::new(release_after(self.route.as_deref()));
        let mut pace = Pacer::new(span);
        while !stop.load(Ordering::Relaxed) {
            if self.follow_route(ring) {
                silence = Silence::new(release_after(self.route.as_deref()));
            }
            ring.fill(&mut buf);
            let hold = silence.hear(&buf, span);
            match (self.pcm, hold) {
                (Some(_), false) => {
                    self.release();
                    eprintln!("slot: audio released after silence");
                }
                (None, true) if Instant::now() >= self.retry_at => {
                    match self.alsa.open_pcm(self.rate, self.route.as_deref()) {
                        Ok(pcm) => {
                            eprintln!("slot: audio reopened at {:.1} ms", crate::boot_time::ms());
                            self.pcm = Some(pcm);
                            self.failing = false;
                        }
                        Err(e) => {
                            if !self.failing {
                                eprintln!("slot: audio: {e}, retrying");
                                self.failing = true;
                            }
                            if self.route.is_some() {
                                self.retry_at = Instant::now() + ROUTE_RETRY;
                            }
                        }
                    }
                }
                _ => {}
            }
            match self.pcm {
                Some(pcm) => {
                    pace.reset();
                    self.report_delay(pcm, ring);
                    if !self.write(pcm, &buf) {
                        self.release();
                        self.failing = true;
                        self.retry_at = Instant::now() + ROUTE_RETRY;
                    }
                }
                None => std::thread::sleep(pace.wait(Instant::now())),
            }
        }
    }

    /// How far behind the picture the headphones play: what the ring holds plus what ALSA,
    /// BlueALSA and the headphones themselves report holding.
    fn report_delay(&mut self, pcm: *mut c_void, ring: &Ring) {
        if self.route.is_none() || self.reported_at.elapsed() < DELAY_REPORT {
            return;
        }
        self.reported_at = Instant::now();
        let mut frames: std::ffi::c_long = 0;
        if unsafe { (self.alsa.delay)(pcm, &mut frames) } < 0 {
            return;
        }
        let ms = |f: f64| 1000.0 * f / f64::from(self.rate);
        eprintln!(
            "slot: audio: headphones {:.0} ms behind ({:.0} ms in the ring, {:.0} ms past it), {} underrun samples",
            ms(frames as f64 + ring.queued_frames() as f64),
            ms(ring.queued_frames() as f64),
            ms(frames as f64),
            ring.underruns()
        );
    }

    fn follow_route(&mut self, ring: &Ring) -> bool {
        if self.route_at.elapsed() < ROUTE_CHECK {
            return false;
        }
        self.route_at = Instant::now();
        let route = route();
        if route == self.route {
            return false;
        }
        eprintln!(
            "slot: audio route {}",
            route.as_deref().unwrap_or("speaker")
        );
        self.release();
        self.route = route;
        self.failing = false;
        self.retry_at = Instant::now();
        reopen_ring(ring, self.rate, self.route.as_deref());
        true
    }

    fn write(&self, pcm: *mut c_void, buf: &[i16]) -> bool {
        let mut written = 0;
        let mut stalled = 0;
        while written < PERIOD_FRAMES {
            let at = written * CHANNELS as usize;
            let frames = unsafe {
                (self.alsa.writei)(
                    pcm,
                    buf[at..].as_ptr() as *const c_void,
                    (PERIOD_FRAMES - written) as u64,
                )
            };
            if frames < 0 {
                let err = unsafe { (self.alsa.recover)(pcm, frames as c_int, 1) };
                if err < 0 {
                    eprintln!("slot: audio: {}", self.alsa.message(err));
                    return false;
                }
                continue;
            }
            if frames == 0 {
                stalled += 1;
                if stalled >= 5 {
                    eprintln!("slot: audio: the device stopped taking sound");
                    return false;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            written += frames as usize;
        }
        true
    }

    fn release(&mut self) {
        if let Some(pcm) = self.pcm.take() {
            unsafe {
                (self.alsa.drop)(pcm);
                (self.alsa.close)(pcm);
            }
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_to_headphones_opens_them_through_a_single_plug() {
        assert_eq!(
            bluetooth_device("bluealsa:DEV=74:77:86:15:3A:A5,PROFILE=a2dp\n").as_deref(),
            Some("plug:SLAVE='bluealsa:DEV=74:77:86:15:3A:A5,PROFILE=a2dp'")
        );
    }

    #[test]
    fn a_route_that_is_not_one_pair_of_headphones_is_ignored() {
        for route in [
            "",
            "default",
            "bluealsa:DEV=74:77:86:15:3A,PROFILE=a2dp",
            "bluealsa:DEV=74:77:86:15:3A:ZZ,PROFILE=a2dp",
            "bluealsa:DEV=74:77:86:15:3A:A5,PROFILE=sco",
            "bluealsa:DEV=74:77:86:15:3A:A5',PROFILE=a2dp",
        ] {
            assert_eq!(
                bluetooth_device(route),
                None,
                "{route:?} was taken as headphones"
            );
        }
    }

    const _HEADPHONES_BUFFER_LESS: () = assert!(BLUETOOTH_LATENCY_US < LATENCY_US);

    #[test]
    fn headphones_hold_their_stream_through_a_rewind() {
        assert_eq!(release_after(None), RELEASE_AFTER);
        assert!(release_after(Some("plug:SLAVE='bluealsa'")) >= Duration::from_secs(30));
    }

    #[test]
    fn headphones_hold_less_sound_ahead_than_the_speaker() {
        assert!(
            super::super::bluetooth_ring_capacity(32_768) < super::super::ring_capacity(32_768)
        );
    }
}
