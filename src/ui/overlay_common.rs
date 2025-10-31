use core::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFRetained, CFString};
use objc2_quartz_core::CATextLayer;
use parking_lot::{Mutex, RwLock};

use crate::actor::app::WindowId;
use crate::common::collections::{HashMap, HashSet};
use crate::sys::window_server::{CapturedWindowImage, WindowServerId, capture_window_image};

#[derive(Debug, Clone)]
pub struct CaptureTask {
    pub window_id: WindowId,
    pub window_server_id: u32,
    pub target_w: usize,
    pub target_h: usize,
}

#[derive(Clone, Copy)]
pub struct RefreshCtx {
    overlay_bits: usize,
    callback: unsafe fn(usize),
}

impl RefreshCtx {
    pub fn new(overlay_ptr: *const c_void, callback: unsafe fn(usize)) -> Self {
        Self {
            overlay_bits: overlay_ptr as usize,
            callback,
        }
    }

    pub fn call(&self) {
        if self.overlay_bits == 0 {
            return;
        }
        unsafe { (self.callback)(self.overlay_bits) };
    }
}

#[derive(Clone)]
pub struct CaptureJob {
    pub task: CaptureTask,
    pub cache: Arc<RwLock<HashMap<WindowId, CapturedWindowImage>>>,
    pub generation: u64,
    pub refresh: RefreshCtx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Enqueued,
    Duplicate,
    ChannelClosed,
}

pub struct CaptureManager {
    sender: Sender<CaptureJob>,
    current_generation: Arc<AtomicU64>,
    in_flight: Arc<Mutex<HashSet<(u64, WindowId)>>>,
}

impl CaptureManager {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        let current_generation = Arc::new(AtomicU64::new(1));
        let in_flight = Arc::new(Mutex::new(HashSet::default()));

        let worker_count = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1))
            .unwrap_or(2)
            .max(2)
            .min(6);

        for _ in 0..worker_count {
            let rx = rx.clone();
            let current_generation = current_generation.clone();
            let in_flight = in_flight.clone();
            thread::spawn(move || worker_loop(rx, current_generation, in_flight));
        }

        Self {
            sender: tx,
            current_generation,
            in_flight,
        }
    }

    pub fn bump_generation(&self) -> u64 {
        self.current_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn current_generation(&self) -> u64 { self.current_generation.load(Ordering::Acquire) }

    pub fn try_mark_in_flight(&self, generation: u64, window_id: WindowId) -> bool {
        let mut set = self.in_flight.lock();
        set.insert((generation, window_id))
    }

    pub fn clear_in_flight(&self, generation: u64, window_id: WindowId) {
        let mut set = self.in_flight.lock();
        set.remove(&(generation, window_id));
    }

    pub fn enqueue(&self, job: CaptureJob) -> EnqueueResult {
        if !self.try_mark_in_flight(job.generation, job.task.window_id) {
            return EnqueueResult::Duplicate;
        }

        let generation = job.generation;
        let window_id = job.task.window_id;

        if self.sender.send(job).is_ok() {
            EnqueueResult::Enqueued
        } else {
            self.clear_in_flight(generation, window_id);
            EnqueueResult::ChannelClosed
        }
    }
}

impl Default for CaptureManager {
    fn default() -> Self { Self::new() }
}

fn worker_loop(
    rx: Receiver<CaptureJob>,
    current_generation: Arc<AtomicU64>,
    in_flight: Arc<Mutex<HashSet<(u64, WindowId)>>>,
) {
    while let Ok(job) = rx.recv() {
        let CaptureJob {
            task,
            cache,
            generation,
            refresh,
        } = job;
        let CaptureTask {
            window_id,
            window_server_id,
            target_w,
            target_h,
        } = task;

        if generation != current_generation.load(Ordering::Acquire) {
            if let Some(mut set) = in_flight.try_lock() {
                set.remove(&(generation, window_id));
            }
            continue;
        }

        let img = capture_window_image(WindowServerId::new(window_server_id), target_w, target_h);

        match img {
            Some(img) => {
                {
                    let mut cache = cache.write();
                    cache.insert(window_id, img);
                }
                if let Some(mut set) = in_flight.try_lock() {
                    set.remove(&(generation, window_id));
                }
                refresh.call();
            }
            None => {
                if let Some(mut set) = in_flight.try_lock() {
                    set.remove(&(generation, window_id));
                }
            }
        }
    }
}

pub struct CachedText {
    text: String,
    attributed: CFRetained<CFString>,
}

impl CachedText {
    pub fn new(text: &str) -> Self {
        let cf = CFString::from_str(text);
        Self {
            text: text.to_owned(),
            attributed: cf,
        }
    }

    pub fn update(&mut self, text: &str) -> bool {
        if self.text == text {
            return false;
        }
        self.text.clear();
        self.text.push_str(text);
        self.attributed = CFString::from_str(text);
        true
    }

    pub fn apply_to(&self, layer: &CATextLayer) {
        let raw = self.attributed.as_ref() as *const AnyObject;
        unsafe { layer.setString(Some(&*raw)) };
    }
}

#[derive(Default)]
pub struct ItemLayerStyle {
    is_selected: Option<bool>,
}

impl ItemLayerStyle {
    pub fn update_selected(&mut self, selected: bool) -> bool {
        if self.is_selected == Some(selected) {
            false
        } else {
            self.is_selected = Some(selected);
            true
        }
    }
}
