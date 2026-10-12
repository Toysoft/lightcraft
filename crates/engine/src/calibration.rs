//! Local calibration jobs: bounded sequential decoding and whole-photo holdout validation.
use crate::camera_profiles::CameraProfile;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};
#[cfg(not(target_arch = "wasm32"))]
use std::{collections::HashSet, path::Path};

#[cfg(not(target_arch = "wasm32"))]
const MAX_FILES: usize = 300;
#[cfg(not(target_arch = "wasm32"))]
const MAX_BYTES: u64 = 512 << 20;
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub running: bool,
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub models: BTreeMap<String, usize>,
    pub warnings: Vec<String>,
    pub results: Vec<Validated>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Validated {
    pub profile: CameraProfile,
    pub holdout: usize,
    pub before: f64,
    pub after: f64,
}
impl Validated {
    pub fn improves(&self) -> bool {
        self.holdout > 0 && self.before.is_finite() && self.after.is_finite() && self.after < self.before
    }
}
#[derive(Default)]
pub struct Job {
    report: Arc<Mutex<Report>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Job {
    pub fn running(&self) -> bool {
        self.report.lock().unwrap_or_else(PoisonError::into_inner).running
    }
    pub fn snapshot(&self) -> Report {
        self.report.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn start(&mut self, paths: Vec<PathBuf>, activity: &crate::activity::Activity) -> Result<(), String> {
        if self.running() {
            return Err("a calibration job is already running".into());
        }
        if paths.is_empty() || paths.len() > 10_000 {
            return Err("choose 1–10,000 files or folders".into());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (paths, activity);
            Err("calibration requires the desktop app".into())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.cancel = Arc::new(AtomicBool::new(false));
            *self.report.lock().unwrap_or_else(PoisonError::into_inner) =
                Report { running: true, stage: "Finding RAWs".into(), ..Default::default() };
            let (report, cancel) = (self.report.clone(), self.cancel.clone());
            let task = activity.start("calibration", "Camera calibration", crate::activity::Cancel::Flag(cancel.clone()));
            let work = move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(paths, &report, &cancel, Some(&task))));
                let mut report = report.lock().unwrap_or_else(PoisonError::into_inner);
                report.running = false;
                report.stage = if cancel.load(Ordering::Acquire) { "Cancelled" } else { "Complete" }.into();
                match result {
                    Ok(Err(e)) => {
                        report.stage = "Failed".into();
                        report.warnings.push(e);
                    }
                    Err(_) => {
                        report.stage = "Failed".into();
                        report.warnings.push("calibration stopped after an internal error".into());
                    }
                    _ => {}
                }
            };
            if let Err(e) = std::thread::Builder::new().name("lc-calibration".into()).spawn(work) {
                self.report.lock().unwrap_or_else(PoisonError::into_inner).running = false;
                return Err(e.to_string());
            }
            Ok(())
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn update(report: &Mutex<Report>, f: impl FnOnce(&mut Report)) {
    f(&mut report.lock().unwrap_or_else(PoisonError::into_inner));
}
#[cfg(not(target_arch = "wasm32"))]
fn warn(report: &Mutex<Report>, text: String) {
    update(report, |r| {
        if r.warnings.len() < 24 {
            r.warnings.push(text);
        }
    });
}
#[cfg(not(target_arch = "wasm32"))]
fn raw_path(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(s.to_ascii_lowercase().as_str(), "arw" | "nef" | "nrw" | "raf" | "rw2" | "cr2" | "cr3" | "pef" | "srw" | "orf" | "dng")
    })
}
#[cfg(not(target_arch = "wasm32"))]
fn with_raw<T>(path: &Path, f: impl FnOnce(&[u8], &mut lightcraft_raw::RawImage) -> Result<T, String>) -> Result<T, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err("RAW exceeds the 512 MiB calibration limit".into());
    }
    let weight = usize::try_from(size).unwrap_or(usize::MAX).saturating_mul(6);
    let _permit = crate::memory::work_gate().acquire(weight);
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("RAW grew beyond the calibration limit".into());
    }
    let mut raw = lightcraft_raw::decode(&bytes).map_err(|e| e.to_string())?;
    raw.opcodes.list3.retain(|op| !op.is_lens_correction());
    f(&bytes, &mut raw)
}
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct Training {
    files: usize,
    pairs: Vec<([f64; 3], [f64; 3])>,
    holdout: Vec<(PathBuf, lightcraft_preview::Hash128)>,
    usable: usize,
    baseline: Option<crate::camera_profiles::ResolvedProfile>,
}
#[cfg(not(target_arch = "wasm32"))]
fn run(paths: Vec<PathBuf>, report: &Mutex<Report>, cancel: &AtomicBool, task: Option<&crate::activity::TaskGuard>) -> Result<(), String> {
    let mut files = Vec::new();
    for path in paths {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if path.is_dir() {
            let walked = crate::walk::files_in(&path, None, crate::walk::Limits { max_depth: 16, max_entries: 20_000 }, raw_path);
            if walked.truncated {
                warn(report, "Folder search stopped at its limit; choose a smaller folder for complete coverage.".into());
            }
            files.extend(walked.files.into_iter().take(10_000usize.saturating_sub(files.len())));
        } else if raw_path(&path) {
            files.push(path);
        }
        if files.len() >= 10_000 {
            warn(report, "Stopped after 10,000 candidate paths; choose a smaller collection or folder for complete coverage.".into());
            break;
        }
    }
    files.sort();
    files.dedup();
    if files.len() > MAX_FILES {
        let step = files.len().div_ceil(MAX_FILES);
        files = files.into_iter().step_by(step).take(MAX_FILES).collect();
        warn(report, "Sampled at most 300 RAWs across the chosen paths to bound memory and processing time.".into());
    }
    if files.is_empty() {
        return Err("No supported RAW files found.".into());
    }
    update(report, |r| {
        r.total = files.len();
        r.stage = "Collecting colour pairs".into();
    });
    warn(report, "Picture-style metadata is unavailable: use one style (Standard recommended), varied light and subjects, with DRO off. Mixed styles cannot be verified automatically.".into());
    let mut models: BTreeMap<String, Training> = BTreeMap::new();
    let mut seen = HashSet::new();
    for (index, path) in files.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = with_raw(path, |bytes, raw| {
            let identity = lightcraft_preview::hash_bytes(bytes);
            if !seen.insert(identity) {
                return Ok(());
            }
            let model = raw.metadata.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or("camera model missing")?.to_string();
            let pairs = crate::camera_preview::profile_pairs(raw, bytes)
                .ok_or("no usable camera-JPEG colour pairs (unsupported, monochrome or insufficient colour)")?;
            let group =
                models.entry(model.clone()).or_insert_with(|| Training { baseline: crate::camera_profiles::resolved(&model), ..Default::default() });
            // Split only usable, distinct whole files. None of a holdout file's pairs enter training.
            group.usable += 1;
            if group.usable.is_multiple_of(5) {
                group.holdout.push((path.clone(), identity));
            } else {
                group.files += 1;
                let step = pairs.len().div_ceil(4000).max(1);
                group.pairs.extend(pairs.into_iter().step_by(step));
            }
            update(report, |r| {
                r.models.insert(model, group.usable);
            });
            Ok(())
        });
        if let Err(e) = result {
            warn(report, format!("Skipped RAW {}: {e}", index + 1));
        }
        update(report, |r| r.done = index + 1);
        if let Some(task) = task {
            task.progress((index + 1) as u64, files.len() as u64);
        }
        crate::memory::release();
    }
    for (model, training) in models {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if training.files < 5 || training.holdout.is_empty() {
            warn(
                report,
                format!("{model}: {} usable RAWs; need at least 5 training photos plus whole-photo holdout (select 7 or more).", training.usable),
            );
            continue;
        }
        update(report, |r| {
            r.stage = format!("Fitting {model}");
            r.done = 0;
            r.total = training.holdout.len();
        });
        let Some((matrix, table)) = crate::camera_preview::fit_profile(&training.pairs) else {
            warn(report, format!("{model}: no usable fit"));
            continue;
        };
        let profile = CameraProfile::new(&model, training.files, training.pairs.len(), matrix, table);
        drop(training.pairs);
        let (mut before, mut after, mut count) = (0.0, 0.0, 0usize);
        update(report, |r| r.stage = format!("Checking held-out {model} photos"));
        for (i, (path, identity)) in training.holdout.iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            match with_raw(path, |bytes, raw| {
                if lightcraft_preview::hash_bytes(bytes) != *identity {
                    return Err("held-out original changed during calibration".into());
                }
                crate::camera_preview::validation_rms(raw, bytes, &profile, training.baseline.as_ref()).ok_or("no aligned validation pixels".into())
            }) {
                Ok((b, a)) if b.is_finite() && a.is_finite() => {
                    before += b;
                    after += a;
                    count += 1;
                }
                _ => warn(report, format!("{model}: a held-out photo could not be evaluated")),
            }
            update(report, |r| r.done = i + 1);
            if let Some(task) = task {
                task.detail(&model);
                task.progress((i + 1) as u64, training.holdout.len() as u64);
            }
            crate::memory::release();
        }
        if count > 0 {
            update(report, |r| r.results.push(Validated { profile, holdout: count, before: before / count as f64, after: after / count as f64 }));
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a local RAW corpus; never uploads or commits photographs"]
    fn calibration_from_local_raws() {
        let manifest = std::env::var("LIGHTCRAFT_CALIBRATION_TEST_FILES").unwrap();
        let paths: Vec<PathBuf> = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
        let report = Mutex::new(Report::default());
        run(paths, &report, &AtomicBool::new(false), None).unwrap();
        let report = report.into_inner().unwrap();
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert!(!report.results.is_empty(), "no model passed collection and holdout validation");
        for result in &report.results {
            assert!(result.profile.files >= 5 && result.holdout > 0);
            assert!(result.before.is_finite() && result.after.is_finite());
        }
    }

    #[test]
    fn malformed_raws_finish_without_results_or_installable_profiles() {
        let directory = std::env::temp_dir().join(format!("lc-calibration-bad-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("bad.ARW");
        std::fs::write(&path, b"not a camera file").unwrap();
        let report = Mutex::new(Report::default());
        run(vec![path], &report, &AtomicBool::new(false), None).unwrap();
        let report = report.into_inner().unwrap();
        assert_eq!(report.done, 1);
        assert!(report.results.is_empty());
        assert!(report.warnings.iter().any(|w| w.contains("Skipped RAW")));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn cancellation_does_not_start_decoding_and_improvement_requires_valid_holdout() {
        let report = Mutex::new(Report::default());
        run(vec![PathBuf::from("must-not-be-read.ARW")], &report, &AtomicBool::new(true), None).unwrap();
        assert_eq!(report.lock().unwrap().done, 0);
        let mut result =
            Validated { profile: CameraProfile::new("Test", 5, 100, lightcraft_color::Mat3::IDENTITY, None), holdout: 1, before: 0.1, after: 0.09 };
        assert!(result.improves());
        result.after = 0.11;
        assert!(!result.improves());
        result.after = f64::NAN;
        assert!(!result.improves());
        result.after = 0.05;
        result.holdout = 0;
        assert!(!result.improves());
    }
}
