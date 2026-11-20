(async () => {
  const state = window.__courseflowCaptcha ||= { token: '', failed: false, ready: false, popped: false };
  if (!state.started) {
    state.started = true;
    const popup = () => {
      // The wrapper calls captchaIns with the popup instance; showing it
      // triggers the challenge load, so do not wait for onReady first.
      if (state.instance && !state.popped) {
        state.popped = true;
        state.instance.popUp();
      }
    };
    const loader = document.createElement('script');
    // Use the same official wrapper as the study page. Its ImageFocusCombine
    // branch selects the 75f9... jigsaw component; the space branch (4da305...)
    // is the click fallback and must not be requested for shared-course study.
    loader.src = 'https://www.zhihuishu.com/assets/ableCaptcha/ableCaptchaCommon.js';
    loader.onerror = () => { state.failed = true; };
    loader.onload = () => {
      try {
        window.initAbleYidunFallback({
          eleCapId: '#courseflow-captcha', width: '320px',
          showPop: true, enableClose: 1, capType: 'ImageFocusCombine',
          onSuccessAble: (_error, payload) => {
            const validate = payload?.data?.validate;
            if (validate) state.token = validate;
            else state.failed = true;
          },
          onErrorAble: () => { state.failed = true; },
          onCloseAble: () => {},
          captchaIns: instance => { state.instance = instance; popup(); },
        });
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
