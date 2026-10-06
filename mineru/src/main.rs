// mineru — convert a file to markdown / latex / docx / html via the
// MinerU parse API (mineru.net/api/v4).
//
// Rushi tool contract: one JSON object on stdin (the arguments), one
// JSON object on stdout (the result). On failure the message goes to
// stderr and the exit code is non-zero; the kernel forwards stderr to
// the model as the tool error.
//
// Requires the MINERU_TOKEN env var (a Bearer token created on the
// mineru.net API-management page).
//
// Arguments:
//   file           (string, required unless url) local path to upload
//   url            (string) remote file URL; skips the upload
//   formats        (array, default ["markdown"]) markdown | latex | docx | html
//   model_version  (string, default "pipeline") pipeline | vlm | MinerU-HTML
//   is_ocr         (bool, default false)
//   enable_formula (bool, default true)
//   enable_table   (bool, default true)
//   language       (string, optional; API default "ch")
//   page_ranges    (string, optional, e.g. "1-10")
//   data_id        (string, optional client identifier)
//   out_dir        (string, default ".")
//   timeout_s      (int, default 900, cap 3600)
//   keep_zip       (bool, default false)
//
// Result:
//   { state, model_version, batch_id | task_id, zip_url,
//     outputs: [{format, path}], images_dir?, zip?, tls_insecure_fallback? }
//
// Output layout in out_dir: <stem>.md / <stem>.tex / <stem>.docx /
// <stem>.html for each requested format, plus the images/ dir the
// markdown and latex reference with relative paths. keep_zip adds
// <stem>.mineru.zip.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const API_ROOT: &str = "https://mineru.net/api/v4";
const POLL_EVERY_S: u64 = 10;
const DEFAULT_TIMEOUT_S: u64 = 900;
const MAX_TIMEOUT_S: u64 = 3600;
const FORMATS: [&str; 4] = ["markdown", "latex", "docx", "html"];

fn main() {
    let args = read_args();
    match run(args) {
        Ok(result) => emit_json(&result),
        Err(msg) => fail(&msg),
    }
}

/// Read the single JSON arguments object from stdin.
fn read_args() -> Value {
    let mut s = String::new();
    use std::io::Read;
    let _ = std::io::stdin().read_to_string(&mut s);
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

/// Emit the single JSON result object on stdout.
fn emit_json(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

/// Print the failure message to stderr and exit non-zero.
fn fail(msg: &str) -> ! {
    eprintln!("mineru: {msg}");
    std::process::exit(1);
}

/// Locate a helper binary. A bundled copy in the tool's own bin dir
/// (placed there by the ext flake) wins; otherwise fall back to PATH.
fn find_bin(name: &str) -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(name);
            if bundled.is_file() {
                return bundled;
            }
        }
    }
    PathBuf::from(name)
}

/// Run a helper binary and return stdout. On a non-zero exit, return
/// the status plus the stderr text as the error.
fn run_cmd(bin: &str, args: &[String]) -> Result<String, String> {
    let p = find_bin(bin);
    let out = Command::new(&p)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn {bin} ({:?}): {e}", p))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let mut msg = format!("{bin} exited with {}", out.status);
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !err.is_empty() {
            msg.push_str(": ");
            msg.push_str(&err);
        }
        return Err(msg);
    }
    Ok(stdout)
}

/// Write the Authorization + Content-Type headers to a file so the
/// token never appears in a process argv.
fn write_headers(path: &Path, token: &str) -> Result<(), String> {
    let body = format!(
        "Authorization: Bearer {token}\nContent-Type: application/json\n"
    );
    fs::write(path, body)
        .map_err(|e| format!("cannot write header file {path:?}: {e}"))
}

/// POST a JSON body to the MinerU API and parse the JSON response.
fn api_post(url: &str, token: &str, body: &Value, work: &Path) -> Result<Value, String> {
    let hdr = work.join("mineru-headers.txt");
    write_headers(&hdr, token)?;
    let payload = serde_json::to_string(body).unwrap();
    let out = run_cmd(
        "curl",
        &[
            "-sS".into(),
            "-X".into(),
            "POST".into(),
            "-H".into(),
            format!("@{}", hdr.display()),
            "--data".into(),
            payload,
            url.to_string(),
        ],
    )
    .map_err(|e| format!("mineru API POST {url}: {e}"))?;
    api_response(&out, url)
}

/// GET from the MinerU API and parse the JSON response.
fn api_get(url: &str, token: &str, work: &Path) -> Result<Value, String> {
    let hdr = work.join("mineru-headers.txt");
    write_headers(&hdr, token)?;
    let out = run_cmd(
        "curl",
        &["-sS".into(), "-H".into(), format!("@{}", hdr.display()), url.to_string()],
    )
    .map_err(|e| format!("mineru API GET {url}: {e}"))?;
    api_response(&out, url)
}

/// Parse an API response body and check the business code.
fn api_response(body: &str, url: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| format!("mineru API {url} returned non-JSON: {e}"))?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("(no msg)");
        return Err(format!("mineru API error code {code}: {msg}"));
    }
    Ok(v)
}

/// PUT a local file to a MinerU upload URL (no Content-Type, per the
/// API docs).
fn upload_file(file: &str, url: &str) -> Result<(), String> {
    let code = run_cmd(
        "curl",
        &[
            "-sS".into(),
            "-X".into(),
            "PUT".into(),
            "-T".into(),
            file.to_string(),
            "-o".into(),
            "/dev/null".into(),
            "-w".into(),
            "%{http_code}".into(),
            url.to_string(),
        ],
    )
    .map_err(|e| format!("file upload to {url}: {e}"))?;
    let code = code.trim();
    if code != "200" {
        return Err(format!("file upload to MinerU failed (http {code})"));
    }
    Ok(())
}

/// Download the result zip. Try TLS-verified first; if the CDN
/// certificate is broken (the MinerU result CDN served an expired
/// certificate as of 2026-10), retry without verification and flag
/// it in the result.
fn download_zip(url: &str, dest: &Path) -> Result<bool, String> {
    let mut base: Vec<String> = vec![
        "-sS".into(),
        "-f".into(),
        "-L".into(),
        "-o".into(),
        dest.to_string_lossy().into_owned(),
        url.to_string(),
    ];
    match run_cmd("curl", &base) {
        Ok(_) => return Ok(false),
        Err(e) => {
            let tls = e.contains("(60)")
                || e.to_lowercase().contains("ssl")
                || e.to_lowercase().contains("certificate");
            if !tls {
                return Err(format!("failed to download the result zip: {e}"));
            }
        }
    }
    base.insert(2, "-k".into());
    run_cmd("curl", &base)
        .map(|_| true)
        .map_err(|e| format!("failed to download the result zip (insecure retry): {e}"))
}

enum Job {
    Batch { id: String, poll_url: String },
    Task { id: String, poll_url: String },
}

/// Poll the job until done / failed / timeout. Return the full_zip_url.
fn poll_job(job: &Job, token: &str, work: &Path, timeout_s: u64) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    loop {
        let url = match job {
            Job::Batch { poll_url, .. } => poll_url.clone(),
            Job::Task { poll_url, .. } => poll_url.clone(),
        };
        let resp = api_get(&url, token, work)?;
        let entry = match job {
            Job::Batch { .. } => resp
                .get("data")
                .and_then(|d| d.get("extract_result"))
                .and_then(|r| r.get(0))
                .ok_or("API response has no extract_result[0]")?,
            Job::Task { .. } => resp.get("data").ok_or("API response has no data")?,
        };
        let state = entry.get("state").and_then(|s| s.as_str()).unwrap_or("");
        match state {
            "done" => {
                return entry
                    .get("full_zip_url")
                    .and_then(|z| z.as_str())
                    .map(|z| z.to_string())
                    .ok_or_else(|| "task done but no full_zip_url in the response".to_string());
            }
            "failed" => {
                let err = entry
                    .get("err_msg")
                    .and_then(|e| e.as_str())
                    .unwrap_or("(no err_msg)");
                return Err(format!("mineru parse task failed: {err}"));
            }
            _ => {} // waiting-file, pending, running, converting
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out after {timeout_s}s waiting for the mineru parse task (last state: {state})"
            ));
        }
        thread::sleep(Duration::from_secs(POLL_EVERY_S));
    }
}

/// The extension and preferred file name for each output format,
/// matching the result zip layout (full.md, full.tex, ...).
fn format_target(fmt: &str) -> (&'static str, Option<&'static str>) {
    match fmt {
        "markdown" => (".md", Some("full.md")),
        "latex" => (".tex", Some("full.tex")),
        "docx" => (".docx", None),
        "html" => (".html", None),
        _ => unreachable!("validated earlier"),
    }
}

/// Collect files under dir ending with ext, tagged with depth.
fn walk_ext(dir: &Path, depth: u32, ext: &str, out: &mut Vec<(u32, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_ext(&p, depth + 1, ext, out);
        } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(ext) {
                out.push((depth, p));
            }
        }
    }
}

/// Find the best file for a format: the preferred name if present,
/// else the shallowest match.
fn find_format_file(dir: &Path, fmt: &str) -> Option<PathBuf> {
    let (ext, preferred) = format_target(fmt);
    let mut cands: Vec<(u32, PathBuf)> = Vec::new();
    walk_ext(dir, 0, ext, &mut cands);
    if let Some(name) = preferred {
        if let Some(hit) = cands
            .iter()
            .find(|(_, p)| p.file_name().and_then(|n| n.to_str()) == Some(name))
        {
            return Some(hit.1.clone());
        }
    }
    cands.sort();
    cands.into_iter().map(|(_, p)| p).next()
}

/// Recursively copy a directory tree.
fn copy_dir(src: &Path, dst: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    if src.is_dir() {
        fs::create_dir_all(dst)
            .map_err(|e| format!("cannot create {dst:?}: {e}"))?;
        for entry in fs::read_dir(src).map_err(|e| format!("cannot read {src:?}: {e}"))? {
            let entry = entry.map_err(|e| format!("read_dir {src:?}: {e}"))?;
            let n = copy_dir(&entry.path(), &dst.join(entry.file_name()))?;
            total += n;
        }
    } else {
        total = fs::copy(src, dst).map_err(|e| format!("cannot copy {src:?} -> {dst:?}: {e}"))?;
    }
    Ok(total)
}

/// Derive the output file stem from a local path or a remote URL.
fn stem_of(file: &str, url: &str) -> String {
    let base = if !file.is_empty() {
        file.to_string()
    } else {
        let path = url.split('?').next().unwrap_or(url);
        path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("mineru").to_string()
    };
    Path::new(&base)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mineru".to_string())
}

fn run(args: Value) -> Result<Value, String> {
    let token = env::var("MINERU_TOKEN").unwrap_or_default();
    if token.trim().is_empty() {
        return Err(
            "MINERU_TOKEN env var is not set. Create a token on the mineru.net API-management page and export MINERU_TOKEN."
                .into(),
        );
    }

    let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if file.is_empty() && url.is_empty() {
        return Err("provide `file` (a local path) or `url` (a remote file URL)".into());
    }
    if !file.is_empty() && !Path::new(&file).is_file() {
        return Err(format!("file not found: {file}"));
    }
    if !url.is_empty()
        && !url.starts_with("http://")
        && !url.starts_with("https://")
    {
        return Err(format!("url must start with http:// or https:// (got: {url:?})"));
    }

    let formats: Vec<String> = if let Some(v) = args.get("formats") {
        let list: Vec<String> = if v.is_array() {
            v.as_array()
                .unwrap()
                .iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.to_string())
                .collect()
        } else if let Some(s) = v.as_str() {
            vec![s.to_string()]
        } else {
            return Err("formats must be an array of strings".into());
        };
        if list.is_empty() {
            return Err("formats must not be empty".into());
        }
        for f in &list {
            if !FORMATS.contains(&f.as_str()) {
                return Err(format!("invalid format {f:?}; valid: {FORMATS:?}"));
            }
        }
        list
    } else {
        vec!["markdown".to_string()]
    };
    let extra: Vec<String> = formats
        .iter()
        .filter(|f| f.as_str() != "markdown")
        .cloned()
        .collect();

    let model_version = args
        .get("model_version")
        .and_then(|v| v.as_str())
        .unwrap_or("pipeline");
    if !matches!(model_version, "pipeline" | "vlm" | "MinerU-HTML") {
        return Err(format!(
            "model_version must be pipeline, vlm, or MinerU-HTML (got: {model_version:?})"
        ));
    }
    let is_ocr = args.get("is_ocr").and_then(|v| v.as_bool()).unwrap_or(false);
    let enable_formula = args.get("enable_formula").and_then(|v| v.as_bool()).unwrap_or(true);
    let enable_table = args.get("enable_table").and_then(|v| v.as_bool()).unwrap_or(true);
    let language = args
        .get("language")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let page_ranges = args
        .get("page_ranges")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let data_id = args
        .get("data_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let mut out_dir = PathBuf::from(
        args.get("out_dir")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("."),
    );
    if !out_dir.is_absolute() {
        out_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(out_dir);
    }
    let timeout_s = args
        .get("timeout_s")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_S)
        .min(MAX_TIMEOUT_S);
    let keep_zip = args.get("keep_zip").and_then(|v| v.as_bool()).unwrap_or(false);

    // Shared parse options for both submit endpoints.
    let mut common = json!({
        "model_version": model_version,
        "is_ocr": is_ocr,
        "enable_formula": enable_formula,
        "enable_table": enable_table,
    });
    if !extra.is_empty() {
        common["extra_formats"] = json!(extra);
    }
    if let Some(l) = language {
        common["language"] = json!(l);
    }

    // Scratch dir for headers, zip, and extraction.
    let start_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let work = env::temp_dir().join(format!("mineru-{}-{start_ns}", std::process::id()));
    fs::create_dir_all(&work).map_err(|e| format!("cannot create work dir {work:?}: {e}"))?;

    // Submit the job: upload flow for local files, URL flow otherwise.
    // The batch endpoint takes is_ocr / data_id inside the file object;
    // the task endpoint takes them at the top level (per the API docs).
    let job = if !file.is_empty() {
        let name = Path::new(&file)
            .file_name()
            .ok_or_else(|| format!("cannot determine the file name for {file:?}"))?
            .to_string_lossy()
            .into_owned();
        let mut file_obj = json!({ "name": name });
        file_obj["is_ocr"] = json!(is_ocr);
        if let Some(pr) = page_ranges {
            file_obj["page_ranges"] = json!(pr);
        }
        if let Some(d) = data_id {
            file_obj["data_id"] = json!(d);
        }
        let mut body = json!({ "files": [file_obj] });
        for (k, v) in common.as_object().unwrap() {
            body[k] = v.clone();
        }
        let resp = api_post(
            &format!("{API_ROOT}/file-urls/batch"),
            &token,
            &body,
            &work,
        )?;
        let batch_id = resp["data"]["batch_id"]
            .as_str()
            .ok_or("API response has no data.batch_id")?
            .to_string();
        let upload_url = resp["data"]["file_urls"][0]
            .as_str()
            .ok_or("API response has no data.file_urls[0]")?;
        upload_file(&file, upload_url)?;
        Job::Batch {
            id: batch_id.clone(),
            poll_url: format!("{API_ROOT}/extract-results/batch/{batch_id}"),
        }
    } else {
        let mut body = json!({ "url": url });
        for (k, v) in common.as_object().unwrap() {
            body[k] = v.clone();
        }
        if let Some(d) = data_id {
            body["data_id"] = json!(d);
        }
        let resp = api_post(&format!("{API_ROOT}/extract/task"), &token, &body, &work)?;
        let task_id = resp["data"]["task_id"]
            .as_str()
            .ok_or("API response has no data.task_id")?
            .to_string();
        Job::Task {
            id: task_id.clone(),
            poll_url: format!("{API_ROOT}/extract/task/{task_id}"),
        }
    };

    let zip_url = poll_job(&job, &token, &work, timeout_s)?;

    // Download and unpack the result.
    let zip_path = work.join("result.zip");
    let insecure = download_zip(&zip_url, &zip_path)?;
    let extract_dir = work.join("extracted");
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("cannot create {extract_dir:?}: {e}"))?;
    run_cmd(
        "unzip",
        &[
            "-q".into(),
            "-o".into(),
            zip_path.to_string_lossy().into_owned(),
            "-d".into(),
            extract_dir.to_string_lossy().into_owned(),
        ],
    )
    .map_err(|e| format!("failed to unpack the result zip: {e}"))?;

    fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {out_dir:?}: {e}"))?;

    // Copy the requested formats out of the zip.
    let stem = stem_of(&file, &url);
    let mut outputs: Vec<Value> = Vec::new();
    for fmt in &formats {
        let src = find_format_file(&extract_dir, fmt)
            .ok_or_else(|| format!("no {fmt} file found in the result zip"))?;
        let (ext, _) = format_target(fmt);
        let dest = out_dir.join(format!("{stem}{ext}"));
        fs::copy(&src, &dest).map_err(|e| format!("cannot write {dest:?}: {e}"))?;
        outputs.push(json!({ "format": fmt, "path": dest.to_string_lossy() }));
    }

    // The markdown and latex reference images/ with paths relative to
    // their own dir, so copy the images alongside.
    let wants_images = formats.iter().any(|f| f == "markdown" || f == "latex");
    let mut result = json!({
        "state": "done",
        "model_version": model_version,
        "zip_url": zip_url,
        "outputs": outputs,
    });
    match &job {
        Job::Batch { id, .. } => result["batch_id"] = json!(id),
        Job::Task { id, .. } => result["task_id"] = json!(id),
    }
    if wants_images {
        let images_src = extract_dir.join("images");
        if images_src.is_dir() {
            let images_dst = out_dir.join("images");
            let n = copy_dir(&images_src, &images_dst)
                .map_err(|e| format!("failed to copy images: {e}"))?;
            result["images_dir"] = json!(images_dst.to_string_lossy());
            let _ = n;
        }
    }
    if insecure {
        result["tls_insecure_fallback"] = json!(true);
    }
    if keep_zip {
        let zip_out = out_dir.join(format!("{stem}.mineru.zip"));
        fs::copy(&zip_path, &zip_out)
            .map_err(|e| format!("cannot write {zip_out:?}: {e}"))?;
        result["zip"] = json!(zip_out.to_string_lossy());
    }

    // Drop the scratch dir.
    let _ = fs::remove_dir_all(&work);

    Ok(result)
}
