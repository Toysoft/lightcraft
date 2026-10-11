/* Suite-owned session coordinator. No user document bytes leave this origin. */
(() => {
  'use strict';
  const app = 'lightcraft';
  if (!['photocraft', 'vectorcraft', 'pdfcraft', 'lightcraft'].includes(app)) return;
  const DB = 'lightcraft-web-sessions', VERSION = 1, STORE = 'sessions', MAX = 512 * 1024 * 1024;
  let resolveOpened; const opened = new Promise(resolve => { resolveOpened = resolve; });
  const key = `craft-session:${app}`;
  const uuid = () => crypto.randomUUID();
  const state = {phase: 'loading', dirty: false, saving: false, error: null, savedAt: null};
  let db, id, record, restore, pending, signature = '', committed = '', candidate = '', timer;
  let hasPrevious = true;
  let disabled = false, holdLock, tick = 0, badge, overlay, lastInput = 0;
  const wake = () => dispatchEvent(new Event('craft:session-wake'));
  function status() {
    if (!badge) return;
    badge.textContent = state.error ? 'Session not saved · Retry' : state.saving || state.dirty ? 'Saving session…' : state.savedAt ? 'Session saved on this device' : 'Session recovery';
    badge.dataset.error = String(!!state.error);
    badge.title = state.error || (state.savedAt ? `Saved ${new Date(state.savedAt).toLocaleTimeString()}. Open session options.` : 'Local session options');
  }
  function fail(error) {
    state.error = String(error?.message || error); state.dirty = true; status();
  }
  const request = req => new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error || Error('Browser storage failed'));
  });
  const done = tx => new Promise((resolve, reject) => {
    tx.oncomplete = resolve; tx.onabort = tx.onerror = () => reject(tx.error || Error('Session write was interrupted'));
  });
  function openDB() {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(DB, VERSION);
      req.onupgradeneeded = () => { req.result.createObjectStore(STORE, {keyPath: 'id'}); req.result.createObjectStore('heads', {keyPath:'id'}); };
      req.onsuccess = () => {
        req.result.onversionchange = () => { req.result.close(); fail('Session storage changed in another tab. Reload to reconnect.'); };
        resolve(req.result);
      };
      req.onerror = () => reject(req.error || Error('Local storage is unavailable'));
      req.onblocked = () => reject(Error('Close older app tabs to update session storage'));
    });
  }
  async function claim(wanted) {
    if (!navigator.locks) return uuid(); // Never share a writer without an ownership lock.
    return new Promise((resolve, reject) => {
      navigator.locks.request(`craft-session:${app}:${wanted}`, {ifAvailable: true}, lock => {
        if (!lock) return resolve(null);
        resolve(wanted);
        return new Promise(release => { holdLock = release; });
      }).catch(reject);
    });
  }
  async function newIdentity(wanted) {
    id = await claim(wanted) || await claim(uuid());
    try { sessionStorage.setItem(key, id); } catch { /* A unique in-memory writer still works. */ }
  }
  function modal(title, message, buttons) {
    overlay?.remove();
    overlay = document.createElement('dialog'); overlay.className = 'craft-session-dialog';
    const h = document.createElement('h2'); h.textContent = title;
    const p = document.createElement('p'); p.textContent = message;
    const actions = document.createElement('div'); actions.className = 'craft-session-actions';
    overlay.append(h, p, actions); document.body.append(overlay);
    for (const [label, action] of buttons) {
      const b = document.createElement('button'); b.textContent = label;
      b.onclick = async () => { b.disabled = true; try { await action(); } catch (e) { fail(e); p.textContent = state.error; b.disabled = false; } };
      actions.append(b);
    }
    overlay.addEventListener('cancel', e => e.preventDefault()); overlay.showModal();
  }
  function closeModal() { overlay?.close(); overlay?.remove(); overlay = null; }
  function begin() { resolveOpened(id || uuid()); restore = null; state.phase = 'ready'; closeModal(); wake(); status(); }
  function select(saved) {
    if (saved.version !== VERSION || !(saved.bytes instanceof ArrayBuffer) || saved.bytes.byteLength > MAX) throw Error('This saved session is incompatible. It has been kept unchanged.');
    resolveOpened(id); record = saved; restore = new Uint8Array(saved.bytes); state.phase = 'restore'; closeModal(); wake();
  }
  function download() {
    const bytes = pending?.bytes || record?.bytes;
    if (!bytes) return;
    const url = URL.createObjectURL(new Blob([bytes], {type:'application/octet-stream'}));
    const a = document.createElement('a'); a.href = url; a.download = `${app}-session.craft-session`; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function retry() {
    if (!db) { location.reload(); return; }
    state.error = null; disabled = false; signature = ''; candidate = ''; status(); wake();
    if (pending) await drain();
  }
  function importCopy() {
    const input = document.createElement('input'); input.type = 'file'; input.accept = '.craft-session';
    input.onchange = async () => {
      try {
        const file = input.files[0]; if (!file) return;
        if (file.size > MAX) throw Error('Recovery copy exceeds 512 MB');
        if (state.dirty || state.saving) throw Error('Wait until the current session is saved before importing a recovery copy');
        const bytes = await file.arrayBuffer();
        if (new TextDecoder().decode(bytes.slice(0,8)) !== 'CRAFTS01') throw Error('Not a *craft recovery copy');
        const importedId = uuid(), savedAt = Date.now();
        await writeRecord({id:importedId,app,version:VERSION,savedAt,bytes}); sessionStorage.setItem(key,importedId); location.reload();
      } catch(e) { fail(e); options(); }
    };
    input.click();
  }
  function options() {
    modal('Session recovery', state.error || 'Documents, tabs and editing history are saved in this browser on this device. Clearing site data removes them. Save or export important work separately.', [
      ['Continue editing', closeModal],
      ['Keep on this device', async () => { const granted = await navigator.storage?.persist?.(); closeModal(); if (!granted) fail('The browser did not grant persistent storage. Keep separate copies of important work.'); }],
      ['Download recovery copy', download],
      ['Import recovery copy', importCopy],
      ['Retry saving', async () => { await retry(); closeModal(); }],
    ]);
  }
  async function boot() {
    const style = document.createElement('link'); style.rel = 'stylesheet'; style.href = '/sessions/session.css'; document.head.append(style);
    badge = document.createElement('button'); badge.className = 'craft-session-status'; badge.setAttribute('aria-live','polite'); badge.onclick = options; document.body.append(badge); status();
    try {
      db = await openDB();
      let previousId; try { previousId = sessionStorage.getItem(key); } catch { /* optional */ }
      const entries = (await request(db.transaction('heads').objectStore('heads').getAll())).filter(r => r.app === app).sort((a,b) => b.savedAt-a.savedAt);
      let skip = false; try { skip = sessionStorage.getItem(key+':skip') === '1'; sessionStorage.removeItem(key+':skip'); } catch {}
      hasPrevious = entries.length > 0 && !skip;
      const reload = performance.getEntriesByType('navigation')[0]?.type === 'reload';
      const own = !skip && reload && entries.find(r => r.id === previousId);
      await newIdentity(own ? previousId : uuid());
      if (own && id === previousId) select(await request(db.transaction(STORE).objectStore(STORE).get(own.id)));
      else if (entries.length && !skip) {
        const saved = entries[0];
        modal('Resume your previous session?', `Saved ${new Date(saved.savedAt).toLocaleString()}. Restore your documents, tabs, view and undo/redo history. Starting fresh keeps the previous recovery copy.`, [
          ['Restore session', async () => select(await request(db.transaction(STORE).objectStore(STORE).get(saved.id)))], ['Start fresh', begin],
        ]);
      } else begin();
    } catch (e) {
      fail(e);
      modal('Session recovery unavailable', `${state.error}. Your existing saved sessions have not been changed.`, [
        ['Retry', () => location.reload()], ['Continue without recovery', () => { disabled = true; begin(); }],
      ]);
    }
  }
  async function writeRecord(value) {
    const tx = db.transaction([STORE,'heads'], 'readwrite'), complete = done(tx);
    try {
      tx.objectStore(STORE).put(value);
      tx.objectStore('heads').put({id:value.id,app,version:VERSION,savedAt:value.savedAt});
      await complete;
    } catch(e) { try { tx.abort(); } catch {} await complete.catch(() => {}); throw e; }
  }
  async function drain() {
    if (state.saving || !pending || disabled || !db) return;
    state.saving = true; status();
    try {
      while (pending) {
        const next = pending; pending = null;
        const value = {id, app, version: VERSION, savedAt: Date.now(), bytes: next.bytes, previous: record?.bytes || null};
        try {
          await writeRecord(value);
        } catch (e) { pending ||= next; throw e; }
        record = value; committed = next.signature; state.savedAt = value.savedAt; state.error = null;
      }
      state.dirty = candidate !== committed;
    } catch (e) { fail(e); }
    finally { state.saving = false; status(); if (state.dirty && !state.error) wake(); }
  }
  globalThis.CraftSession = {
    version: VERSION, opened,
    hasPrevious: () => hasPrevious,
    startFreshOnNextLoad: () => { sessionStorage.setItem(key+':skip','1'); },
    ready: () => state.phase === 'ready',
    take: () => { const value = restore; restore = null; return value || new Uint8Array(); },
    restored: (ok, error) => {
      if (ok) { state.phase = 'ready'; state.savedAt = record?.savedAt; status(); wake(); }
      else {
        state.phase = 'failed'; fail(error);
        const buttons = [['Download recovery copy', download], ['Start fresh', async () => { holdLock?.(); await newIdentity(uuid()); record = null; begin(); }]];
        if (record?.previous) buttons.unshift(['Try previous checkpoint', () => select({...record, bytes:record.previous, previous:null})]);
        modal('Could not restore this session', `${error}. The recovery copy has been kept unchanged.`, buttons);
      }
    },
    shouldSave: sig => {
      if (disabled || state.phase !== 'ready') return false;
      candidate = sig;
      if (sig === committed) { state.dirty = !!pending || state.saving; status(); return false; }
      if (sig === signature) return false;
      state.dirty = true; status();
      // Repaint drives native capture even after the editor becomes idle. Never serialize mid-drag.
      if (performance.now()-tick < 400 || performance.now()-lastInput < 50) {
        clearTimeout(timer); timer = setTimeout(wake, 450); return false;
      }
      tick = performance.now(); signature = sig; return true;
    },
    save: bytes => {
      if (disabled || state.phase !== 'ready') return;
      if (bytes.byteLength > MAX) { signature = ''; fail('This session exceeds the 512 MB recovery limit. Save your documents before leaving. History has not been truncated.'); return; }
      pending = {bytes: bytes.slice().buffer, signature}; void drain();
    },
    failed: error => { signature = ''; fail(error); },
    status: () => ({...state, app, id}),
  };
  for (const name of ['pointerdown','pointerup','keyup','input','change','wheel']) addEventListener(name, () => { lastInput = performance.now(); if (state.phase === 'ready' && !disabled) state.dirty = true; status(); wake(); setTimeout(wake, 500); }, true);
  addEventListener('beforeunload', e => { if (state.dirty || state.saving) { e.preventDefault(); e.returnValue = ''; } });
  addEventListener('visibilitychange', () => { if (document.hidden) wake(); });
  addEventListener('pageshow', e => { if (e.persisted) wake(); });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot, {once:true}); else void boot();
})();
