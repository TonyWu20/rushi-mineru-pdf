# rushi-mineru-pdf

The `mineru` rushi tool: convert a PDF (or doc / ppt / xls / image)
to markdown, latex, docx, or html through the MinerU parse API
(mineru.net). The tool uploads the file, polls the parse task,
downloads the result zip, and writes the requested formats to disk.

## Tool

One-shot Rust binary that speaks the rushi tool ABI (JSON object on
stdin, one JSON object on stdout, non-zero exit plus stderr on
failure). It shells out to `curl` and `unzip`, which are bundled next
to the binary in the Nix-built package and looked up on PATH in an
in-tree build.

Prerequisite: the `MINERU_TOKEN` env var, a Bearer token created on
the mineru.net API-management page (the token-based 精准解析 API,
`/api/v4`). The Agent 轻量解析 API (`/api/v1/agent`, no token) is
markdown-only and capped at 10 MB / 20 pages, so it cannot serve the
latex export and is not used.

### Arguments

| arg            | type   | default     | meaning                                                    |
|----------------|--------|-------------|------------------------------------------------------------|
| file           | string | —           | local file to parse. Required unless `url` is given.       |
| url            | string | —           | remote file URL to parse instead of uploading a local file |
| formats        | array  | `["markdown"]` | output formats: markdown, latex, docx, html            |
| model_version  | string | `pipeline`  | MinerU model: pipeline or vlm. vlm failed on the test file, so pipeline is the default (see `docs/design.md`) |
| is_ocr         | bool   | `false`     | enable OCR                                                 |
| enable_formula | bool   | `true`      | formula recognition                                        |
| enable_table   | bool   | `true`      | table recognition                                          |
| language       | string | API default | language hint for OCR (`ch` / `en` / ...)                  |
| page_ranges    | string | —           | e.g. `"1-10"` or `"2,4-6"`                                 |
| data_id        | string | —           | client identifier for the parse object                     |
| out_dir        | string | `.`         | directory for the written outputs                          |
| timeout_s      | int    | `900`       | max wait for the parse task (cap 3600)                     |
| keep_zip       | bool   | `false`     | also write the raw result zip next to the outputs          |

### Result

```json
{
  "state": "done",
  "model_version": "pipeline",
  "batch_id": "…",
  "zip_url": "https://cdn-mineru…",
  "outputs": [
    {"format": "markdown", "path": "/abs/out/reviewer_pdf.md"},
    {"format": "latex", "path": "/abs/out/reviewer_pdf.tex"}
  ],
  "images_dir": "/abs/out/images",
  "tls_insecure_fallback": true
}
```

Output layout in `out_dir`: `<stem>.md` / `<stem>.tex` /
`<stem>.docx` / `<stem>.html` for each requested format, plus the
`images/` dir when markdown or latex is requested (both reference
`images/…` with paths relative to their own dir). The `zip` key
appears with `keep_zip`. `tls_insecure_fallback` is `true` when the
result-zip download had to skip TLS verification (the MinerU result
CDN served an expired certificate as of 2026-10; the tool tries the
verified download first and retries with `-k` only on a TLS error).

## Crates

- `mineru` the tool binary plus its `tool.toml` manifest.

In-tree cargo dev (curl and unzip on PATH):

    cargo build --release --manifest-path mineru/Cargo.toml

Or the Nix dev shell: `nix develop` (rust toolchain plus curl and
unzip on PATH).

## Nix packaging

`flake.nix` is a Mode B ext flake (see
`rust-unix-harness/docs/reference/nix/ext-flake-authoring.md`). It
exposes one package per system:

| package  | $out layout                                   |
|----------|----------------------------------------------|
| `mineru` | `$out/mineru/tool.toml` + `$out/mineru/bin/{mineru,curl,unzip}` |

The package declares `meta.rushi.entry = "mineru"` (rushi#13), so a
consumer's `lib.mkRushi` fills the `extension_tool_paths` config at
eval time.

Consumer wiring (`rushi-config`-style flake):

    rushi.external_tools = [ mineruFlake.packages.${system}.mineru ];

## Test asset

`reviewer_pdf.pdf` (58 pages) is the end-to-end test input. The
recorded run: `pipeline` succeeded (markdown + latex + 36 images),
`vlm` failed twice with `parsing failed, please try again later`.
