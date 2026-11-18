// Settings and pairing, in the desktop's bordered sections.
import { useState } from 'preact/hooks';

import { resetAll } from './store.js';
import { pairWith, standalone } from './sync.js';
import { Section } from './ui.jsx';

function ago(t) {
  if (!t) return '尚未同步';
  const s = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (s < 60) return '刚刚同步';
  if (s < 3600) return `${Math.round(s / 60)} 分钟前同步`;
  if (s < 86400) return `${Math.round(s / 3600)} 小时前同步`;
  return `上次同步：${new Date(t).toLocaleDateString()}`;
}

/** Pairing for a phone that has no token yet (e.g. a new home-screen app). */
export function PairCard({ doSync }) {
  const [text, setText] = useState('');
  const [error, setError] = useState(null);
  const pair = async (value) => {
    try {
      setError(null);
      await pairWith(value);
      await doSync();
    } catch (e) {
      setError(e.message);
    }
  };
  const paste = async () => {
    try {
      const t = await navigator.clipboard.readText();
      setText(t);
      await pair(t);
    } catch (_) {
      setError('读不到剪贴板：长按下面的输入框选「粘贴」，再点「配对」。');
    }
  };
  return (
    <Section
      title="和电脑配对"
      desc={
        standalone()
          ? '主屏幕上的 App 和浏览器的数据是分开的，需要再配对一次：在电脑 pic_yomi →「设置 → 手机同步」点配对链接旁的「复制」，再回到这里粘贴。'
          : '在电脑 pic_yomi →「设置 → 手机同步」用相机扫二维码；或者复制配对链接，回到这里粘贴。'
      }
    >
      <button class="btn primary wide" onClick={paste}>
        从剪贴板粘贴配对链接
      </button>
      <div class="field-row">
        <input placeholder="或者粘贴到这里" value={text} autocapitalize="none" autocorrect="off" onInput={(e) => setText(e.currentTarget.value)} />
        <button class="btn outline" onClick={() => pair(text)} disabled={!text.trim()}>
          配对
        </button>
      </div>
      {error && <p class="error">{error}</p>}
    </Section>
  );
}

export function Settings({ syncState, doSync, paired }) {
  return (
    <div class="settings">
      <Section title="同步" desc="复习记录先存在手机上，离线也能背；联网后自动和电脑同步。">
        <div class="status-row">
          <span class={`dot ${!paired ? 'off' : syncState.error ? 'warn' : 'ok'}`} />
          <span>{!paired ? '还没有和电脑配对' : syncState.busy ? '同步中…' : ago(syncState.last)}</span>
        </div>
        {paired && syncState.error && <p class="error">{syncState.error}</p>}
        {paired && (
          <button class="btn outline" onClick={doSync} disabled={syncState.busy}>
            立即同步
          </button>
        )}
      </Section>
      {!paired && <PairCard doSync={doSync} />}
      <Section title="本机数据" desc="清除后会从云端重新下载；还没同步的复习会丢失。">
        <button
          class="btn danger-outline"
          onClick={async () => {
            if (confirm('清除本机缓存的词单和复习记录？未同步的复习会丢失。')) {
              await resetAll();
              location.reload();
            }
          }}
        >
          清除本机数据
        </button>
      </Section>
    </div>
  );
}
