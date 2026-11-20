# Captcha probe

A Playwright-based lab tool for the ZHS Yidun widget. It answers one question:
**which captcha component does the official study page select, and what challenge
does that component present?**

It is deliberately not part of the runtime. The production host still uses the
native CDP browser service; this tool is only for live-page diagnostics and for
reproducing the component-selection behaviour without an account.

## What it wires

The probe installs a **full no-op route**:

- The official `stuStudy` HTML is served at the official origin.
- `ableCaptchaCommon.js` and `yidun-captcha.js` are served from the official
  public assets.
- Only Yidun/captcha hosts are allowed to make real requests.
- Every other script, stylesheet, XHR, fetch, image and media request is
  fulfilled with an empty response and counted.
- No account cookie, platform token or save request is used; the page's study
  app scripts are no-ops.

The probe then calls either the official wrapper
(`initAbleYidunFallback`) or the raw loader (`initNECaptcha`) and reports the
popup class, prompt, requested captcha id and errors. It never prints tokens or
cookies.

## Result (2026-11-20)

| `capType` / component | Requested id | Popup |
| --- | --- | --- |
| `ImageFocusCombine` (official study page) | `75f9f716460a422f89a628f50fd8cc2b` | `yidun--jigsaw` |
| `space` (fallback) | `4da3050565514a35a50541b0e1f54538` | `yidun--space` |

The official study page's `capinitAbleYidunFallback` passes
`capType: 'ImageFocusCombine'`; the wrapper therefore selects the jigsaw
component. `4da305...` is the `space` fallback and must not be used as the
study-page component.

This corrects the earlier assumption that `4da305...` was the study page's
captcha id. The module's original `75f9...` was the right jigsaw component;
0.3.13's switch to `4da305...` was the regression that produced the click
challenge.

## Usage

Use a system Chromium (the tool does not download a browser):

```sh
cd tools/captcha-probe
npm install
node probe.mjs --component study --page official
node probe.mjs --component space --page official
node probe.mjs --component raw75 --page empty
```

Options:

| Option | Default | Meaning |
| --- | --- | --- |
| `--component study` | `study` | Official wrapper, `capType: 'ImageFocusCombine'` |
| `--component space` | | Official wrapper, `capType: 'space'` |
| `--component raw75` | | Raw `initNECaptcha`, jigsaw id |
| `--component raw4da` | | Raw `initNECaptcha`, space id |
| `--page official` | `official` | Serve the official `stuStudy` HTML |
| `--page empty` | | Serve the module's minimal empty document |
| `--runs N` | `1` | Repeat the probe; Yidun risk can vary |
| `--timeout MS` | `15000` | Popup wait |
| `--chromium PATH` | `CF_CHROMIUM` or `/usr/bin/chromium` | Executable |
| `--screenshot DIR` | | Optional screenshots for inspection |

Output is JSON, one object per run, plus a compact summary. A run is successful
when the popup class is observed; the challenge may still be `space` depending
on the component being tested.

## Playwright or production CDP?

Playwright is a good fit for this lab because `page.route` makes a full no-op
route short and readable. It is not a drop-in replacement for the host's browser
service:

- the service already uses a system Chromium and native CDP, and its tests
  exercise that path;
- `playwright-core` adds a Node dependency only for diagnostics;
- production needs a tiny set of CDP operations, while the probe needs rich
  request interception;
- adopting Playwright for production should be a separate decision with
  container size, version pinning and test coverage.

Use this tool when the official page changes, when a new challenge type
appears, or when checking whether a component id is the main component or a
fallback.
