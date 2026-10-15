//! Bluetooth headphones. A worker runs System/bluetooth.sh off the game thread and keeps
//! a view of what it last reported; the app reads the view and asks for jobs.

#[cfg(feature = "device")]
use std::sync::{mpsc, Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Headphones {
    pub mac: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtTask {
    Starting,
    Stopping,
    Scanning,
    Pairing,
    Connecting,
    Speaker,
    Forgetting,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BtView {
    pub powered: bool,
    pub paired: Vec<Headphones>,
    pub found: Vec<Headphones>,
    /// The headphones' own volume, 0..=127, when they let slot set it.
    pub volume: Option<u8>,
    pub busy: Option<(BtTask, Option<String>)>,
    pub failed: Option<String>,
    pub generation: u64,
}

impl BtView {
    pub fn connected(&self, mac: &str) -> bool {
        self.paired.iter().any(|h| h.mac == mac && h.connected)
    }

    pub fn busy_with(&self, mac: &str) -> Option<BtTask> {
        match &self.busy {
            Some((task, Some(m))) if m == mac => Some(*task),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtJob {
    Up,
    Down,
    Status,
    Scan,
    Pair(String),
    Connect(String),
    Speaker,
    Forget(String),
    Volume(u8),
}

impl BtJob {
    pub fn task(&self) -> Option<(BtTask, Option<String>)> {
        Some(match self {
            BtJob::Up => (BtTask::Starting, None),
            BtJob::Down => (BtTask::Stopping, None),
            BtJob::Scan => (BtTask::Scanning, None),
            BtJob::Pair(mac) => (BtTask::Pairing, Some(mac.clone())),
            BtJob::Connect(mac) => (BtTask::Connecting, Some(mac.clone())),
            BtJob::Speaker => (BtTask::Speaker, None),
            BtJob::Forget(mac) => (BtTask::Forgetting, Some(mac.clone())),
            BtJob::Status | BtJob::Volume(_) => return None,
        })
    }

    pub fn args(&self) -> Vec<String> {
        let (verb, arg) = match self {
            BtJob::Up => ("up", None),
            BtJob::Down => ("down", None),
            BtJob::Status => ("status", None),
            BtJob::Scan => ("scan", Some(SCAN_SECONDS.to_string())),
            BtJob::Pair(mac) => ("pair", Some(mac.clone())),
            BtJob::Connect(mac) => ("connect", Some(mac.clone())),
            BtJob::Speaker => ("speaker", None),
            BtJob::Forget(mac) => ("forget", Some(mac.clone())),
            BtJob::Volume(v) => ("volume", Some(v.to_string())),
        };
        std::iter::once(verb.to_string()).chain(arg).collect()
    }
}

pub const SCAN_SECONDS: u32 = 12;

pub trait Bluetooth: Send {
    fn ask(&mut self, job: BtJob);

    fn view(&self) -> BtView;

    fn generation(&self) -> u64;
}

/// slot's 0..=100 level to the headphones' own 0..=127 and back.
pub fn to_headphones(level: u8) -> u8 {
    ((u32::from(level.min(100)) * 127 + 50) / 100) as u8
}

pub fn from_headphones(volume: u8) -> u8 {
    ((u32::from(volume.min(127)) * 100 + 63) / 127) as u8
}

/// A curly apostrophe or other glyph the label font lacks would print as a box.
pub fn shown_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        })
        .collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub powered: Option<bool>,
    pub paired: Option<Vec<Headphones>>,
    pub found: Vec<Headphones>,
    pub volume: Option<u8>,
    pub error: Option<String>,
}

pub fn parse_reply(text: &str) -> Reply {
    let mut r = Reply::default();
    for line in text.lines() {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "power" => r.powered = Some(rest == "on"),
            "device" => {
                let mut f = rest.splitn(3, ' ');
                if let (Some(mac), Some(on), name) = (f.next(), f.next(), f.next()) {
                    r.paired.get_or_insert_with(Vec::new).push(Headphones {
                        mac: mac.to_string(),
                        name: name.unwrap_or(mac).to_string(),
                        connected: on == "1",
                    });
                }
            }
            "found" => {
                if let Some((mac, name)) = rest.split_once(' ') {
                    r.found.push(Headphones {
                        mac: mac.to_string(),
                        name: name.to_string(),
                        connected: false,
                    });
                }
            }
            "volume" => r.volume = rest.parse().ok(),
            "error" => r.error = Some(rest.to_string()),
            _ => {}
        }
    }
    r
}

/// Applies one finished job's reply to the view.
pub fn absorb(view: &mut BtView, job: &BtJob, reply: Reply, ok: bool) {
    if let Some(p) = reply.powered {
        view.powered = p;
    }
    match job {
        BtJob::Status => {
            view.paired = reply.paired.unwrap_or_default();
            view.volume = reply.volume;
        }
        BtJob::Scan => {
            view.found = reply
                .found
                .into_iter()
                .filter(|f| !view.paired.iter().any(|p| p.mac == f.mac))
                .collect();
        }
        BtJob::Volume(_) => {
            if reply.volume.is_some() {
                view.volume = reply.volume;
            }
        }
        BtJob::Down => {
            for h in &mut view.paired {
                h.connected = false;
            }
            view.volume = None;
        }
        _ => {}
    }
    view.failed = match (ok, reply.error) {
        (true, _) => None,
        (false, Some(e)) => Some(e),
        (false, None) => Some("Bluetooth did not answer".into()),
    };
    if matches!(job, BtJob::Status | BtJob::Volume(_)) && ok {
        view.failed = None;
    }
}

/// A Bluetooth that does nothing: hosts without a radio, and tests that do not care.
#[derive(Default)]
pub struct NoBluetooth;

impl Bluetooth for NoBluetooth {
    fn ask(&mut self, _job: BtJob) {}

    fn view(&self) -> BtView {
        BtView::default()
    }

    fn generation(&self) -> u64 {
        0
    }
}

/// Headphones that answer at once, for tests and for working on the menu off the device.
/// Clones share one set of headphones, so a test can keep a handle on what the app holds.
#[derive(Clone, Default)]
pub struct ScriptedBluetooth(std::sync::Arc<std::sync::Mutex<Script>>);

#[derive(Default)]
pub struct Script {
    pub view: BtView,
    pub nearby: Vec<Headphones>,
    pub asked: Vec<BtJob>,
    pub refuse_pairing: bool,
    pub unreachable: bool,
}

impl Script {
    fn link(&mut self, mac: &str) -> bool {
        if self.unreachable {
            return false;
        }
        for h in &mut self.view.paired {
            h.connected = h.mac == mac;
        }
        let linked = self.view.paired.iter().any(|h| h.mac == mac);
        if linked && self.view.volume.is_none() {
            self.view.volume = Some(127);
        }
        linked
    }

    /// Something changed on the headphones' side, as a status read would find it.
    pub fn changed(&mut self) {
        self.view.generation += 1;
    }
}

impl ScriptedBluetooth {
    pub fn with_nearby(nearby: &[(&str, &str)]) -> Self {
        let s = ScriptedBluetooth::default();
        s.script().nearby = nearby
            .iter()
            .map(|(mac, name)| Headphones {
                mac: mac.to_string(),
                name: name.to_string(),
                connected: false,
            })
            .collect();
        s
    }

    pub fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn asked(&self) -> Vec<BtJob> {
        self.script().asked.clone()
    }
}

impl Bluetooth for ScriptedBluetooth {
    fn ask(&mut self, job: BtJob) {
        let mut guard = self.script();
        let script = &mut *guard;
        script.asked.push(job.clone());
        let ok = match &job {
            BtJob::Up => {
                script.view.powered = true;
                true
            }
            BtJob::Down => {
                script.view.powered = false;
                for h in &mut script.view.paired {
                    h.connected = false;
                }
                script.view.volume = None;
                true
            }
            BtJob::Status => true,
            BtJob::Scan => {
                let paired = &script.view.paired;
                script.view.found = script
                    .nearby
                    .iter()
                    .filter(|n| !paired.iter().any(|p| p.mac == n.mac))
                    .cloned()
                    .collect();
                true
            }
            BtJob::Pair(mac) => match script.nearby.iter().find(|n| &n.mac == mac).cloned() {
                Some(h) if !script.refuse_pairing => {
                    if !script.view.paired.iter().any(|p| &p.mac == mac) {
                        script.view.paired.push(h);
                    }
                    script.view.found.retain(|f| &f.mac != mac);
                    script.link(mac)
                }
                _ => false,
            },
            BtJob::Connect(mac) => script.link(mac),
            BtJob::Speaker => {
                for h in &mut script.view.paired {
                    h.connected = false;
                }
                script.view.volume = None;
                true
            }
            BtJob::Forget(mac) => {
                script.view.paired.retain(|p| &p.mac != mac);
                true
            }
            BtJob::Volume(n) => {
                if script.view.paired.iter().any(|h| h.connected) {
                    script.view.volume = Some(*n);
                }
                true
            }
        };
        let v = &mut script.view;
        v.busy = None;
        v.failed = (!ok).then(|| match job {
            BtJob::Pair(_) => "Could not pair".into(),
            _ => "Could not connect".into(),
        });
        v.generation += 1;
    }

    fn view(&self) -> BtView {
        self.script().view.clone()
    }

    fn generation(&self) -> u64 {
        self.script().view.generation
    }
}

#[cfg(feature = "device")]
struct Worker {
    tx: mpsc::Sender<BtJob>,
    shared: Arc<Mutex<BtView>>,
}

#[cfg(feature = "device")]
impl Bluetooth for Worker {
    fn ask(&mut self, job: BtJob) {
        if let Some(task) = job.task() {
            let mut v = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            v.busy = Some(task);
            v.failed = None;
            v.generation += 1;
        }
        let _ = self.tx.send(job);
    }

    fn view(&self) -> BtView {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn generation(&self) -> u64 {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }
}

#[cfg(feature = "device")]
fn helper(job: &BtJob) -> (Reply, bool) {
    let root = std::env::var_os("SLOT_ROOT").unwrap_or_else(|| "/mnt/sdcard".into());
    let script = std::path::Path::new(&root).join("System/bluetooth.sh");
    match std::process::Command::new("/bin/sh")
        .arg(script)
        .args(job.args())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(out) => (
            parse_reply(&String::from_utf8_lossy(&out.stdout)),
            out.status.success(),
        ),
        Err(e) => (
            Reply {
                error: Some(format!("bluetooth.sh would not start: {e}")),
                ..Reply::default()
            },
            false,
        ),
    }
}

#[cfg(feature = "device")]
pub fn bluetooth() -> Box<dyn Bluetooth> {
    let (tx, rx) = mpsc::channel::<BtJob>();
    let shared = Arc::new(Mutex::new(BtView::default()));
    let view = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("slot-bluetooth".into())
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                // A volume swipe sends a burst: only the last level matters, and a queue
                // of status reads behind a slow job is one read.
                while let Ok(next) = rx.try_recv() {
                    match (&job, &next) {
                        (BtJob::Volume(_), BtJob::Volume(_)) | (BtJob::Status, BtJob::Status) => {
                            job = next
                        }
                        _ => {
                            run(&view, &job);
                            job = next;
                        }
                    }
                }
                run(&view, &job);
            }
        });
    if spawned.is_err() {
        return Box::new(NoBluetooth);
    }
    Box::new(Worker { tx, shared })
}

#[cfg(feature = "device")]
fn run(view: &Mutex<BtView>, job: &BtJob) {
    let (reply, ok) = helper(job);
    if !ok {
        eprintln!(
            "slot: bluetooth: {} failed: {}",
            job.args().join(" "),
            reply.error.as_deref().unwrap_or("no reason given")
        );
    }
    let changes_links = !matches!(job, BtJob::Status | BtJob::Volume(_) | BtJob::Scan);
    let status = changes_links.then(|| helper(&BtJob::Status));
    let mut v = view.lock().unwrap_or_else(|e| e.into_inner());
    absorb(&mut v, job, reply, ok);
    if let Some((status, true)) = status {
        let failed = v.failed.take();
        absorb(&mut v, &BtJob::Status, status, true);
        v.failed = failed;
    }
    if v.busy.is_some() && v.busy == job.task() {
        v.busy = None;
    }
    v.generation += 1;
}

#[cfg(not(feature = "device"))]
pub fn bluetooth() -> Box<dyn Bluetooth> {
    Box::new(NoBluetooth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_reply_lists_the_pairs_and_the_volume() {
        let r = parse_reply(
            "power on\ndevice 74:77:86:15:3A:A5 1 Rad’s Airpods Pro 3 - Find My\ndevice AA:BB:CC:DD:EE:01 0 Desk Speaker\nvolume 60\n",
        );
        assert_eq!(r.powered, Some(true));
        let paired = r.paired.unwrap();
        assert_eq!(paired.len(), 2);
        assert_eq!(paired[0].name, "Rad’s Airpods Pro 3 - Find My");
        assert!(paired[0].connected && !paired[1].connected);
        assert_eq!(r.volume, Some(60));
    }

    #[test]
    fn a_status_with_nothing_paired_still_clears_the_list() {
        let mut v = BtView {
            paired: vec![Headphones {
                mac: "AA:BB:CC:DD:EE:01".into(),
                name: "Old".into(),
                connected: true,
            }],
            ..BtView::default()
        };
        absorb(&mut v, &BtJob::Status, parse_reply("power on\n"), true);
        assert!(v.paired.is_empty());
    }

    #[test]
    fn a_scan_leaves_out_headphones_already_paired() {
        let mut v = BtView {
            paired: vec![Headphones {
                mac: "AA:BB:CC:DD:EE:01".into(),
                name: "Mine".into(),
                connected: false,
            }],
            ..BtView::default()
        };
        let r = parse_reply("found AA:BB:CC:DD:EE:01 Mine\nfound AA:BB:CC:DD:EE:02 New Pair\n");
        absorb(&mut v, &BtJob::Scan, r, true);
        assert_eq!(v.found.len(), 1);
        assert_eq!(v.found[0].name, "New Pair");
    }

    #[test]
    fn a_failed_job_keeps_the_helpers_reason() {
        let mut v = BtView::default();
        let r = parse_reply("error Could not connect\n");
        absorb(
            &mut v,
            &BtJob::Connect("AA:BB:CC:DD:EE:01".into()),
            r,
            false,
        );
        assert_eq!(v.failed.as_deref(), Some("Could not connect"));
    }

    #[test]
    fn the_volume_scale_maps_both_ends_and_survives_a_round_trip() {
        assert_eq!((to_headphones(0), to_headphones(100)), (0, 127));
        assert_eq!((from_headphones(0), from_headphones(127)), (0, 100));
        for level in (0..=100).step_by(5) {
            assert_eq!(from_headphones(to_headphones(level)), level);
        }
    }

    #[test]
    fn a_name_prints_with_glyphs_the_font_has() {
        assert_eq!(
            shown_name("Rad’s Airpods Pro 3 – Find My"),
            "Rad's Airpods Pro 3 - Find My"
        );
    }

    #[test]
    fn every_job_names_its_helper_verb() {
        let mac = "AA:BB:CC:DD:EE:01".to_string();
        assert_eq!(
            BtJob::Connect(mac.clone()).args(),
            ["connect", mac.as_str()]
        );
        assert_eq!(BtJob::Volume(40).args(), ["volume", "40"]);
        assert_eq!(BtJob::Scan.args(), ["scan", "12"]);
        assert_eq!(BtJob::Speaker.args(), ["speaker"]);
    }
}
