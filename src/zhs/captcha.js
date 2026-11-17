(async () => {
  const state = window.__courseflowCaptcha ||= { token: '', failed: false, ready: false, popped: false };
  if (!state.started) {
    state.started = true;
    const popup = () => {
      if (state.ready && state.instance && !state.popped) {
        state.popped = true;
        state.instance.popUp();
      }
    };
    const loader = document.createElement('script');
    loader.src = 'https://cstaticdun.126.net/load.min.js';
    loader.onerror = () => { state.failed = true; };
    loader.onload = () => {
      try {
        window.initNECaptcha({
          element: '#courseflow-captcha', captchaId: '75f9f716460a422f89a628f50fd8cc2b',
          width: '320px', mode: 'popup', lang: 'zh-CN', protocol: 'https',
          onReady: () => { state.ready = true; popup(); },
          onVerify: (error, data) => {
            if (!error && data?.validate) state.token = data.validate;
            else state.failed = true;
          },
          onError: () => { state.failed = true; },
        }, instance => { state.instance = instance; popup(); }, () => { state.failed = true; });
      } catch { state.failed = true; }
    };
    document.head.appendChild(loader);
  }
  const inspect = () => {
    if (state.token) return { state: 'verified', token: state.token };
    if (state.failed) return { state: 'failed' };
    const popup = document.querySelector('.yidun--popup');
    if (!popup || popup.classList.contains('yidun--loading')) return null;
    if (popup.classList.contains('yidun--space')) return { state: 'unsupported' };
    if (!popup.classList.contains('yidun--jigsaw')) return { state: 'unsupported' };
    if (window.__courseflowCaptchaWaitToken) return null;
    const background = document.querySelector('.yidun_bg-img');
    const piece = document.querySelector('.yidun_jigsaw');
    const slider = document.querySelector('.yidun_slider');
    if (!background?.complete || !piece?.complete || !background.naturalWidth || !piece.naturalWidth || !slider) return null;
    const bg = background.getBoundingClientRect(), target = piece.getBoundingClientRect(), handle = slider.getBoundingClientRect();
    if (bg.width <= 0 || handle.width <= 0) return null;
    return {
      state: 'puzzle', background_url: background.currentSrc || background.src, target_url: piece.currentSrc || piece.src,
      natural_width: background.naturalWidth, width: bg.width, piece_left: target.x - bg.x,
      slider_x: handle.x + handle.width / 2, slider_y: handle.y + handle.height / 2,
    };
  };
  const deadline = Date.now() + 4000;
  do {
    const result = inspect();
    if (result) return result;
    await new Promise(resolve => setTimeout(resolve, 100));
  } while (Date.now() < deadline);
  return { state: 'loading' };
})()
