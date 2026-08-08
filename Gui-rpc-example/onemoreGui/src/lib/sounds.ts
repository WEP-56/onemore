// 通知提示音:命令成功/失败时播放。资源在 public/assets。

let audioEl: HTMLAudioElement | null = null;

function ensureAudio(src: string): HTMLAudioElement {
  if (!audioEl) {
    audioEl = new Audio();
    audioEl.preload = "auto";
  }
  audioEl.src = src;
  return audioEl;
}

export function playSuccessSound() {
  try {
    void ensureAudio("assets/success-notification.mp3").play();
  } catch {
    // ignore
  }
}

export function playErrorSound() {
  try {
    void ensureAudio("assets/error-notification.mp3").play();
  } catch {
    // ignore
  }
}
