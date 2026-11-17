const api = typeof browser !== 'undefined' ? browser : chrome;

function send(type, payload) {
  return new Promise((resolve, reject) => {
    api.runtime.sendMessage({ type, payload }, (res) => {
      if (api.runtime.lastError) return reject(new Error(api.runtime.lastError.message));
      res && res.ok ? resolve(res.data) : reject(new Error(res ? res.error : 'no response'));
    });
  });
}

const $ = (id) => document.getElementById(id);

(async () => {
  try {
    const h = await send('health');
    const s = await send('stats');
    $('state').textContent = h.ocr_ready ? '已连接桌面程序' : '已连接，但 OCR 模型未下载（桌面程序 → 设置）';
    $('due').textContent = s.due_now;
    $('today').textContent = s.reviewed_today;
    $('total').textContent = s.total;
    $('stats').hidden = false;
  } catch (e) {
    $('state').textContent = '没有连上 pic_yomi 桌面程序，请先打开它。';
    $('state').className = 'err';
  }
})();

$('toggle').addEventListener('click', async () => {
  const [tab] = await api.tabs.query({ active: true, currentWindow: true });
  if (tab) api.tabs.sendMessage(tab.id, { type: 'toggleText' });
  window.close();
});

$('review').addEventListener('click', () => {
  api.tabs.create({ url: 'http://127.0.0.1:7331/app/' });
  window.close();
});
