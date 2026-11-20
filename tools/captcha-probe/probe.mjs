import fs from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { chromium } from 'playwright-core';

const PAGE_URL = 'https://studyvideoh5.zhihuishu.com/stuStudy';
const ABLE_URL = 'https://www.zhihuishu.com/assets/ableCaptcha/ableCaptchaCommon.js';
const YIDUN_FALLBACK_URL = 'https://www.zhihuishu.com/assets/ableCaptcha/yidun-captcha.js';

const JIGSAW_ID = '75f9f716460a422f89a628f50fd8cc2b';
const SPACE_ID = '4da3050565514a35a50541b0e1f54538';

const EMPTY_HTML = '<!doctype html><html lang="zh-CN"><meta charset="utf-8">' +
  '<title>验证码</title><div id="captchaYidun"></div><div id="courseflow-captcha"></div></html>';

const CAPTCHA_HOSTS = [
  'cstaticdun.126.net',
  'dun.163.com',
  'necaptcha.nosdn.127.net',
  'nos.netease.com',
  'nstool.netease.com',
];

const CAPTCHA_PATHS = [
  /\/load\.min\.js$/,
  /\/ableCaptcha\//,
  /\/yidun-captcha\.js$/,
];

function parseArgs(argv) {
  const options = {
    component: 'study',
    page: 'official',
    runs: 1,
    timeout: 15_000,
    chromium: process.env.CF_CHROMIUM || '/usr/bin/chromium',
    screenshot: '',
  };
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key === '--component') { options.component = value; index += 1; }
    else if (key === '--page') { options.page = value; index += 1; }
    else if (key === '--runs') { options.runs = Number(value); index += 1; }
    else if (key === '--timeout') { options.timeout = Number(value); index += 1; }
    else if (key === '--chromium') { options.chromium = value; index += 1; }
    else if (key === '--screenshot') { options.screenshot = value; index += 1; }
    else if (key === '--help' || key === '-h') { options.help = true; }
    else throw new Error(`未知参数：${key}`);
  }
  if (!['study', 'space', 'raw75', 'raw4da'].includes(options.component)) {
    throw new Error('--component 只支持 study、space、raw75、raw4da');
  }
  if (!['official', 'empty'].includes(options.page)) {
    throw new Error('--page 只支持 official、empty');
  }
  if (!Number.isInteger(options.runs) || options.runs < 1 || options.runs > 20) {
    throw new Error('--runs 必须是 1–20 的整数');
  }
  if (!Number.isFinite(options.timeout) || options.timeout < 3_000 || options.timeout > 60_000) {
    throw new Error('--timeout 必须是 3000–60000 毫秒');
  }
  return options;
}

function usage() {
  return `用法：node probe.mjs [选项]

  --component study|space|raw75|raw4da  组件选择（默认 study）
  --page official|empty                 官方 HTML 或空文档（默认 official）
  --runs N                              重复次数，1–20（默认 1）
  --timeout MS                          等待弹窗毫秒数（默认 15000）
  --chromium PATH                       Chromium 路径（默认 CF_CHROMIUM 或 /usr/bin/chromium）
  --screenshot DIR                      可选，保存截图到目录
  -h, --help                            显示帮助
`;
}

async function fetchText(url) {
  const response = await fetch(url, {
    redirect: 'follow',
    signal: AbortSignal.timeout(20_000),
    headers: {
      'user-agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 ' +
        '(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36',
      'accept-language': 'zh-CN,zh;q=0.9',
    },
  });
  if (!response.ok) throw new Error(`获取 ${url} 失败：HTTP ${response.status}`);
  return response.text();
}

function isCaptchaUrl(raw) {
  let url;
  try { url = new URL(raw); } catch { return false; }
  if (CAPTCHA_HOSTS.some((host) => url.hostname === host || url.hostname.endsWith(`.${host}`))) return true;
  return CAPTCHA_PATHS.some((pattern) => pattern.test(url.pathname));
}

function captchaIds(urls) {
  const found = new Set();
  for (const raw of urls) {
    const match = /[?&]id=([0-9a-f]{32})(?:&|$)/i.exec(raw);
    if (match) found.add(match[1]);
  }
  return [...found];
}

async function installNoopRoute(page, assets, mode) {
  const counters = { document: 0, wrapper: 0, yidun: 0, blocked: 0, blockedByType: {} };
  const yidunUrls = [];
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = request.url();
    const type = request.resourceType();

    if (url === PAGE_URL) {
      counters.document += 1;
      return route.fulfill({
        status: 200,
        contentType: 'text/html; charset=utf-8',
        body: mode === 'empty' ? EMPTY_HTML : assets.html,
      });
    }
    if (url === ABLE_URL) {
      counters.wrapper += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/javascript; charset=utf-8',
        body: assets.able,
      });
    }
    if (url === YIDUN_FALLBACK_URL) {
      counters.wrapper += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/javascript; charset=utf-8',
        body: assets.yidunFallback,
      });
    }
    if (isCaptchaUrl(url)) {
      counters.yidun += 1;
      if (yidunUrls.length < 40) yidunUrls.push(url);
      return route.continue();
    }

    counters.blocked += 1;
    counters.blockedByType[type] = (counters.blockedByType[type] || 0) + 1;

    if (type === 'script') {
      return route.fulfill({ status: 200, contentType: 'application/javascript; charset=utf-8', body: '' });
    }
    if (type === 'stylesheet') {
      return route.fulfill({ status: 200, contentType: 'text/css; charset=utf-8', body: '' });
    }
    if (type === 'xhr' || type === 'fetch') {
      return route.fulfill({ status: 200, contentType: 'application/json; charset=utf-8', body: '{}' });
    }
    if (type === 'document') {
      return route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: EMPTY_HTML });
    }
    return route.fulfill({ status: 204, body: '' });
  });
  return { counters, yidunUrls };
}

async function probeOnce(assets, options, run) {
  const browser = await chromium.launch({
    executablePath: options.chromium,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage', '--autoplay-policy=no-user-gesture-required'],
  });
  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, locale: 'zh-CN' });
    const page = await context.newPage();
    const pageErrors = [];
    page.on('pageerror', (error) => {
      if (pageErrors.length < 5) pageErrors.push(String(error).slice(0, 400));
    });

    const { counters, yidunUrls } = await installNoopRoute(page, assets, options.page);
    await page.goto(PAGE_URL, { waitUntil: 'domcontentloaded', timeout: 30_000 });
    await page.evaluate(() => {
      window.__probe = { token: false, error: null, captchaInstance: false };
      if (!document.getElementById('courseflow-captcha')) {
        const element = document.createElement('div');
        element.id = 'courseflow-captcha';
        document.body.appendChild(element);
      }
    });

    let captchaId = '';
    if (options.component === 'study' || options.component === 'space') {
      await page.addScriptTag({ content: assets.able });
      const capType = options.component === 'study' ? 'ImageFocusCombine' : 'space';
      await page.evaluate((value) => {
        window.initAbleYidunFallback({
          eleCapId: '#courseflow-captcha',
          element: '#courseflow-captcha',
          width: '320px',
          showPop: true,
          enableClose: 1,
          capType: value,
          customTexts: { popupTitle: '为保障您的学习安全，请进行验证' },
          onSuccessAble: (_error, payload) => {
            window.__probe.token = Boolean(payload?.data?.validate);
          },
          onErrorAble: (error) => {
            window.__probe.error = String(error?.msg ?? error).slice(0, 300);
          },
          onCloseAble: () => {},
          captchaIns: (instance) => {
            window.__probe.captchaInstance = true;
            try { instance.popUp(); } catch (error) {
              window.__probe.error = `popUp: ${String(error).slice(0, 200)}`;
            }
          },
        });
      }, capType);
    } else {
      const rawId = options.component === 'raw75' ? JIGSAW_ID : SPACE_ID;
      await page.addScriptTag({ url: 'https://cstaticdun.126.net/load.min.js' });
      await page.waitForFunction(() => typeof window.initNECaptcha === 'function', null, { timeout: 10_000 });
      await page.evaluate((id) => {
        window.initNECaptcha({
          element: '#courseflow-captcha',
          captchaId: id,
          width: '320px',
          mode: 'popup',
          lang: 'zh-CN',
          protocol: 'https',
          onReady: () => {
            try { window.__probeInstance?.popUp(); } catch (error) {
              window.__probe.error = `popUp: ${String(error).slice(0, 200)}`;
            }
          },
          onVerify: (error, data) => {
            if (!error && data?.validate) window.__probe.token = true;
            else window.__probe.error = String(error?.msg ?? error ?? 'onVerify').slice(0, 300);
          },
          onError: () => { window.__probe.error = 'onError'; },
        }, (instance) => {
          window.__probe.captchaInstance = true;
          window.__probeInstance = instance;
          try { instance.popUp(); } catch (error) {
            window.__probe.error = `popUp: ${String(error).slice(0, 200)}`;
          }
        }, () => { window.__probe.error = 'onload error'; });
      }, rawId);
      captchaId = rawId;
    }

    await page.waitForTimeout(options.timeout);
    const snapshot = await page.evaluate(() => {
      const popup = document.querySelector('.yidun--popup');
      const text = (document.body?.innerText || '').replace(/\s+/g, ' ').trim();
      return {
        popup_class: popup?.className ?? null,
        prompt: text.slice(0, 500),
        probe: window.__probe,
      };
    });

    if (options.screenshot) {
      await fs.mkdir(options.screenshot, { recursive: true });
      await page.screenshot({
        path: path.join(options.screenshot, `probe-${options.component}-${options.page}-${run}.png`),
      });
    }

    const popup = snapshot.popup_class || '';
    const challenge = popup.includes('yidun--jigsaw') ? 'jigsaw'
      : popup.includes('yidun--space') ? 'space'
        : 'unknown';

    return {
      component: options.component,
      page: options.page,
      challenge,
      popup_class: snapshot.popup_class,
      prompt: snapshot.prompt,
      requested_ids: [...new Set([...captchaIds(yidunUrls), captchaId].filter(Boolean))],
      token_received: snapshot.probe?.token === true,
      probe_error: snapshot.probe?.error || null,
      captcha_instance: snapshot.probe?.captchaInstance === true,
      page_errors: pageErrors,
      noop: {
        document: counters.document,
        wrapper: counters.wrapper,
        yidun_requests: counters.yidun,
        blocked: counters.blocked,
        blocked_by_type: counters.blockedByType,
      },
    };
  } finally {
    await browser.close();
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    process.stdout.write(usage());
    return;
  }
  const [html, able, yidunFallback] = await Promise.all([
    fetchText(PAGE_URL),
    fetchText(ABLE_URL),
    fetchText(YIDUN_FALLBACK_URL),
  ]);
  const assets = { html, able, yidunFallback };
  for (let run = 1; run <= options.runs; run += 1) {
    const result = await probeOnce(assets, options, run);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  }
}

main().catch((error) => {
  process.stderr.write(`captcha probe 失败：${error?.stack || error}\n`);
  process.exitCode = 1;
});
