(() => {
  if (/^(passport|login)\./.test(location.hostname)) return { auth_redirect: true };
  const observed = window.__courseflowObserved || {};
  for (const response of Object.values(observed)) if (response.error) return response.code === 'platform_login_expired' ? { authentication_required: true } : { blocked: response.error };
  if (!window.__courseflowStarted) {
    // 官方播放器会在首次 timeupdate 后把 videoFlag/watchHistoryFlag 复位，不能把它们当成持久的就绪状态。
    const ranges = window.videoStartEndmap;
    const rangeCount = typeof ranges?.size === 'function' ? ranges.size() : Number(ranges?.size || 0);
    const ready = { history: observed['/getWatchHistory']?.loaded === true, videos: Array.isArray(window.videoInfo) && videoInfo.length > 0, timeline: rangeCount > 0, authorization: window.courseEndStatus != null, player: typeof window.onPaly === 'function' };
    const waiting = Object.keys(ready).find((key) => !ready[key]);
    if (waiting) return { loading: true, phase: waiting, readiness: ready };
    const watched = new Set((window.watchHistoryBack || []).map(String));
    let offset = 0;
    let target;
    for (let index = 0; index < videoInfo.length && !target; index++) {
      const video = videoInfo[index];
      const duration = Number(video.duration);
      if (!Number.isFinite(duration) || duration <= 0) return { blocked: '官方回放返回了无效视频时长，请在平台核对' };
      const minutes = Math.ceil(duration / 60);
      for (let minute = 1; minute <= minutes; minute++) {
        if (!watched.has(String(offset + minute))) { target = { id: video.id, index, position: (minute - 1) * 60 }; break; }
      }
      offset += minutes;
    }
    if (!target) return { blocked: '观看记录已满，但平台尚未确认签到，请到官方页面核对' };
    onPaly(target.id, target.index);
    window.seekTime = target.position;
    window.__courseflowStarted = true;
  }
  const video = document.querySelector('#forFollowBackDiv video');
  if (!video) return { loading: true, phase: 'media' };
  video.muted = true;
  video.playbackRate = 1;
  if (video.paused && !video.ended) video.play().catch((error) => { window.__courseflowPlayError = ['NotAllowedError', 'NotSupportedError', 'AbortError'].includes(error.name) ? error.name : 'PlaybackError'; });
  const media = { ready_state: video.readyState ?? null, network_state: video.networkState ?? null, paused: video.paused, ended: video.ended, error_code: video.error?.code ?? null, play_error: window.__courseflowPlayError || null };
  if (media.error_code) return { blocked: `回放媒体加载失败（媒体错误 ${media.error_code}），请在官方页面核对`, media };
  if (media.play_error === 'NotSupportedError') return { blocked: '浏览器无法播放此回放媒体，请在官方页面核对格式或资源状态', media };
  return { key: String(window.curVideoId), position: Math.floor(video.currentTime), percent: document.querySelector('.qiandao-num')?.textContent || '0', media };
})()
