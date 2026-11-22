const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let status = null;
const seen = new Set(); // "author:seq" of messages already in the log
let lastDay = '';

function fail(e) {
  showNotice(String(e));
}

// ---- Status-driven screens ---------------------------------------------------------

function render() {
  const s = status;
  const screen = !s.me.name ? 'setup' : s.peer ? 'chat' : 'pair';
  for (const id of ['setup', 'pair', 'chat']) $(id).hidden = id !== screen;

  if (screen === 'pair') renderPair(s);
  if (screen === 'chat') renderChatHeader(s);
  renderDialog(s.pairing);
  showNotice(s.notice);
  updateDelivery();
}

function renderPair(s) {
  $('pair-me').textContent = s.me.name;
  const list = $('nearby');
  list.replaceChildren(
    ...s.nearby.map((p) => {
      const li = document.createElement('li');
      const name = document.createElement('span');
      name.textContent = p.name || 'Unnamed';
      const btn = document.createElement('button');
      btn.textContent = 'Pair';
      btn.disabled = !!s.pairing;
      btn.onclick = () => invoke('pair_with', { id: p.id }).catch(fail);
      li.append(name, btn);
      return li;
    }),
  );
  $('nearby-empty').hidden = s.nearby.length > 0;
}

function renderChatHeader(s) {
  $('peer-name').textContent = s.peer.name || 'Your peer';
  const link = $('link');
  link.classList.toggle('on', s.connected);
  link.textContent = s.connected
    ? 'Connected'
    : 'Not connected · messages send when you’re both online';
}

function renderDialog(p) {
  $('dialog').hidden = !p;
  if (!p) return;
  const who = p.name || 'Someone';
  $('dialog-code').textContent = p.code;
  if (p.incoming) {
    $('dialog-title').textContent = `${who} wants to pair`;
    $('dialog-body').textContent = 'Only accept if their screen shows the same code.';
    $('dialog-no').textContent = 'Decline';
    $('dialog-yes').hidden = false;
  } else {
    $('dialog-title').textContent = `Waiting for ${who}`;
    $('dialog-body').textContent = 'Check that their screen shows this code, then they’ll accept.';
    $('dialog-no').textContent = 'Cancel';
    $('dialog-yes').hidden = true;
  }
}

function showNotice(text) {
  $('notice').hidden = !text;
  $('notice-text').textContent = text || '';
}

// ---- Messages ----------------------------------------------------------------------

function addMessage(m) {
  const key = `${m.author}:${m.seq}`;
  if (seen.has(key)) return;
  seen.add(key);

  const log = $('log');
  log.querySelector('.empty')?.remove();
  const nearBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
  const mine = m.author === status.me.id;

  const when = new Date(m.ts);
  const day = when.toDateString();
  if (day !== lastDay) {
    lastDay = day;
    const d = document.createElement('div');
    d.className = 'day';
    d.textContent = when.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' });
    log.append(d);
  }

  const el = document.createElement('div');
  el.className = 'msg' + (mine ? ' mine' : '');
  el.dataset.seq = m.seq;
  el.textContent = m.text;
  const meta = document.createElement('span');
  meta.className = 'meta';
  meta.textContent = when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  el.append(meta);
  log.append(el);

  if (mine) updateDelivery();
  if (nearBottom || mine) log.scrollTop = log.scrollHeight;
}

function updateDelivery() {
  if (!status) return;
  for (const el of document.querySelectorAll('.msg.mine')) {
    const delivered = Number(el.dataset.seq) <= status.peer_has;
    el.classList.toggle('delivered', delivered);
    el.classList.toggle('pending', !delivered);
  }
}

function showEmptyLog() {
  const log = $('log');
  if (log.children.length) return;
  const p = document.createElement('p');
  p.className = 'empty';
  p.textContent = 'No messages yet. Say hi.';
  log.append(p);
}

// ---- Wiring ------------------------------------------------------------------------

$('name-form').addEventListener('submit', (e) => {
  e.preventDefault();
  invoke('set_name', { name: $('name-input').value }).catch(fail);
});

$('rename').onclick = () => {
  const name = prompt('Your name', status.me.name);
  if (name) invoke('set_name', { name }).catch(fail);
};

$('unpair').onclick = () => {
  if (confirm(`Unpair from ${status.peer.name}? Your history stays on this computer.`)) {
    invoke('unpair').catch(fail);
  }
};

$('dialog-yes').onclick = () => invoke('answer_pair', { accept: true });
$('dialog-no').onclick = () => invoke('answer_pair', { accept: false });
$('notice-close').onclick = () => invoke('dismiss_notice');

const text = $('text');
function autosize() {
  text.style.height = 'auto';
  text.style.height = text.scrollHeight + 'px';
}
text.addEventListener('input', autosize);
text.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    $('compose').requestSubmit();
  }
});
$('compose').addEventListener('submit', async (e) => {
  e.preventDefault();
  const value = text.value.trim();
  if (!value) return;
  text.value = '';
  autosize();
  try {
    addMessage(await invoke('send_message', { text: value }));
  } catch (err) {
    text.value = value;
    fail(err);
  }
});

async function init() {
  // Listen first so nothing that happens during loading is missed.
  await listen('tightbeam', ({ payload }) => {
    if (payload.kind === 'status') {
      status = payload.data;
      render();
    } else if (payload.kind === 'message' && status) {
      addMessage(payload.data);
    }
  });
  status = await invoke('get_status');
  render();
  for (const m of await invoke('get_messages')) addMessage(m);
  showEmptyLog();
  $('log').scrollTop = $('log').scrollHeight;
}

init().catch(fail);
