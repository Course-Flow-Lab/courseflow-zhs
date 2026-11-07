(() => {
  if (/^(passport|login)\./.test(location.hostname)) return { auth_redirect: true };
  const observed = window.__courseflowObserved || {};
  for (const response of Object.values(observed)) if (response.error) return response.code === 'platform_login_expired' ? { authentication_required: true } : { blocked: response.error };
  if (!observed['/getWatchHistory']?.loaded) return { loading: true };
  if (!window.videoFlag || !window.videoInfo?.length || window.courseEndStatus === null || !window.videoStartEndmap?.size?.()) return { loading: true };
  if (!window.__courseflowStarted) {
    const watched = new Set((window.watchHistoryBack || []).map(String));
    let offset = 0;
    let target;
    for (let index = 0; index < videoInfo.length && !target; index++) {
      const video = videoInfo[index];
      const minutes = Math.ceil(Number(video.duration) / 60);
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
  if (!video) return { loading: true };
  video.muted = true;
  video.playbackRate = 1;
  if (video.paused && !video.ended) video.play().catch(() => {});
  return { key: String(window.curVideoId), position: Math.floor(video.currentTime), percent: document.querySelector('.qiandao-num')?.textContent || '0' };
})()
