# Design decisions

Status: decisions made on 2026-10-06 while building this ext.
Each entry records what was chosen and why, against the evidence in
the MinerU API docs (https://mineru.net/apiManage/docs) and the
live runs of 2026-10-06.

## D1. Token-based 精准解析 API (v4), not the Agent 轻量解析 API (v1)

The task asks for pdf → latex or markdown. The Agent API
(`/api/v1/agent/parse/*`, no token) outputs markdown only, is capped
at 10 MB / 20 pages, and disables table / formula recognition. The
v4 API supports the latex / docx / html exports via `extra_formats`,
200 MB / 200 pages, and table / formula options. The user provides
`MINERU_TOKEN`, so the v4 API is the right surface.

## D2. Local files go through the upload flow

`POST /api/v4/extract/task` takes a file URL and does not accept a
direct file upload (the docs say so explicitly). For a local file
the flow is:

1. `POST /api/v4/file-urls/batch` — apply for a signed upload URL
   (response: `batch_id` + `file_urls`).
2. `PUT` the file to the signed URL (no Content-Type header, per
   the docs; http 200 = success).
3. The server auto-submits the parse task; poll
   `GET /api/v4/extract-results/batch/{batch_id}` until
   `state=done` (states: waiting-file, pending, running,
   converting, done, failed).

A remote-URL input instead uses `POST /api/v4/extract/task`
(`url` + the same parse options) and polls
`GET /api/v4/extract/task/{task_id}`. The tool supports both.

## D3. `model_version` default is `pipeline`, not `vlm`

The API docs recommend `vlm` and default the parameter to
`pipeline`. Live runs of 2026-10-06 on `reviewer_pdf.pdf` (58
pages): `vlm` failed twice with `parsing failed, please try again
later` (error class -60010 / -60007), `pipeline` succeeded within
about two minutes. The docs give no rule for when vlm fails, so the
tool defaults to the reliable `pipeline` and documents `vlm` as the
accuracy option the model can retry with.

## D4. Shell out to bundled curl + unzip; no new cargo deps

The Nix store vendors the cargo registry (the `cargo-vendor-dir`
derivations), and sandboxed builds cannot fetch new crates.
`rushi-web-access` sets the precedent: a Rust tool that shells out
to a bundled helper binary (obscura), bundled into the tool's own
`bin/` dir by the ext flake, found by the binary via its own exe
dir before PATH. This tool follows the same pattern with `curl`
(HTTP + signed-URL PUT + zip download) and `unzip` (result
archive). The crate therefore depends only on `serde_json`, pinned
to the same serde 1.0.229 / serde_json 1.0.151 as
rushi-web-access's lock file, which the store's vendor dirs already
carry.

## D5. The token never appears in a process argv

The Authorization header is written to a scratch file in the
process temp dir and passed to curl as `-H @file`. curl shows up
in the process table without the token; the scratch dir is
removed at the end of the run.

## D6. TLS-verified download first, `-k` retry flagged in the result

As of 2026-10-06 the MinerU result CDN
(`cdn-mineru.openxlab.org.cn`) serves a certificate that expired
on 2026-10-02 (verified by decoding the DER validity period:
2026-03-18 to 2026-10-02). A verified download fails with curl
exit 60. The tool tries the verified download, and only when the
failure is TLS-related retries with `-k` and records
`tls_insecure_fallback: true` in the result, so the agent sees the
degradation instead of a silent one.

## D7. Output layout: flat files plus `images/`

The result zip carries `full.md`, `full.tex` (when latex is
requested), `full.docx` / `full.html` (when requested),
`*_content_list*.json`, `*_model.json`, `layout.json`, and an
`images/` dir. The markdown and the latex both reference the
images with paths relative to their own dir (`images/<hash>.jpg`;
36 refs on the test file), so the tool copies the `images/` dir
next to the outputs whenever markdown or latex is requested. The
requested formats are written to `<out_dir>/<stem>.md` /
`<stem>.tex` / `<stem>.docx` / `<stem>.html`; the raw zip only
with `keep_zip`.

## D8. Timeout budget

`tool.toml` sets `timeout_ms = 1800000` (30 min). The internal
`timeout_s` arg (default 900 s, cap 3600 s) bounds the polling
loop. The kernel route enforces the tool-level timeout either way.

## D9. `data_id` and `is_ocr` placement is endpoint-specific

The API docs put `file.data_id` and `file.is_ocr` inside the
`files` entry of `POST /api/v4/file-urls/batch`, but top-level
`data_id` on `POST /api/v4/extract/task`. The tool assembles each
body to match its endpoint.
