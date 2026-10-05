// Regression for v0.2.2 fixes:
//  A) event replay into a FRESH GUI process (bridge last_seq starts at 0,
//     subscribe from_seq=1 -> daemon replays all history; daemon stays alive
//     across GUI restarts). GUI must be relaunched before this script runs.
//  B) task detail opens (no black screen) with ledger/checkpoints/events
//  C) emergency stop while running -> resume all -> banner cleared
//  D) mock rounds narrative correct, no cost-drift warnings, steer delivered
// NOTE: ASCII-only.
const { chromium } = require('playwright-core');
(async () => {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
  let page = browser.contexts()[0]?.pages()[0];
  if (!page) throw new Error('no page via CDP');
  // WebView2 cold start: playwright first sees about:blank, wait for tauri nav
  for (let i = 0; i < 40 && !page.url().includes('tauri.localhost'); i++) {
    await page.waitForTimeout(500).catch(() => {});
  }
  console.log('page-url:', page.url());
  const shot = (n) => page.screenshot({ path: `C:\\dev\\uitest\\${n}.png` });
  const errors = [];
  page.on('pageerror', e => errors.push('PAGEERROR: ' + e.message));
  page.on('dialog', d => d.accept());
  const feedText = () => page.evaluate(() => document.querySelector('.events')?.innerText ?? '');

  // make sure we start on the task-list view
  await page.getByRole('button', { name: /返回任务列表/ }).click().catch(() => {});
  await page.waitForTimeout(300);

  const runId = 'rg' + Date.now().toString().slice(-6);
  const title = 'regress ' + runId;

  // --- A: replay. Fresh GUI bridge: last_seq=0 -> from_seq=1 -> daemon replays
  // history. Feed starts empty and must fill asynchronously (subscribe RTT +
  // replay burst). Poll up to 25s.
  let a;
  for (let i = 0; i < 25; i++) {
    a = await page.evaluate(() => ({
      feedEvents: document.querySelectorAll('.events .ev').length,
      cards: document.querySelectorAll('.card').length,
      header: !!document.querySelector('header'),
    }));
    if (a.feedEvents > 0) break;
    await page.waitForTimeout(1000);
  }
  console.log('A replay:', JSON.stringify(a), a.feedEvents > 0 ? 'PASS' : 'FAIL');
  await shot('rg-a-replay');

  // --- D: fresh task
  await page.getByRole('button', { name: '新建任务' }).click();
  await page.waitForTimeout(300);
  await page.locator('.modal input').first().fill(title);
  await page.locator('.modal textarea').fill('demo: finish in 3 rounds, one section per round');
  await page.locator('.modal input').nth(1).fill('C:\\dev\\maestro-demo');
  await page.getByRole('button', { name: '派活' }).click();

  // steer as soon as steer input appears (working state)
  let steerToast = 'no-steer-input';
  for (let i = 0; i < 12; i++) {
    await page.waitForTimeout(900);
    if (await page.locator('.card .steer').count() > 0) {
      await page.locator('.card .steer').first().fill('steer: remember cost data');
      await page.locator('.card').first().getByRole('button', { name: '发送' }).click();
      await page.waitForTimeout(600);
      steerToast = await page.evaluate(() => document.querySelector('.toast')?.textContent ?? '(none)');
      break;
    }
  }
  console.log('D steer-toast:', steerToast);

  // --- C: emergency stop WHILE running -> resume all -> banner clears
  await page.waitForTimeout(1200);
  await page.getByRole('button', { name: '急停' }).click();
  await page.waitForTimeout(1200);
  const c1 = await page.evaluate(() => !!document.querySelector('.banner'));
  await page.getByRole('button', { name: '收件箱' }).click();
  await page.waitForTimeout(600);
  await page.getByRole('button', { name: '全部恢复' }).click();
  await page.waitForTimeout(1500);
  // return from inbox view back to the task list (cards live there)
  await page.getByRole('button', { name: /返回任务列表/ }).click().catch(() => {});
  await page.waitForTimeout(400);
  const c2 = await page.evaluate(() => ({
    banner: !!document.querySelector('.banner'),
    toast: document.querySelector('.toast')?.textContent ?? null,
  }));
  console.log('C emergency:', JSON.stringify({ stopped: c1, ...c2 }), (c1 && !c2.banner) ? 'PASS' : 'FAIL');
  await shot('rg-c-resumed');

  // wait for completion after resume (3 rounds x ~1.2s + worker spawn)
  let done = false;
  for (let i = 0; i < 45; i++) {
    const cards = await page.evaluate(() =>
      Array.from(document.querySelectorAll('.card')).map(c => c.innerText));
    if (cards.some(t => t.includes(title) && (t.includes('done') || t.includes('完成')))) { done = true; break; }
    await page.waitForTimeout(1500);
  }
  const feed = await feedText();
  // Semantics: steering_delivered carries the round the steer was consumed
  // into; rendered text adds +1 ("takes effect next round"). Assert delivery,
  // not a hardcoded round.
  const checks = {
    done,
    round1Narrative: feed.includes('第 1 轮：') ,
    noCostDrift: !feed.includes('对账漂移'),
    steerDelivered: feed.includes('轻推已投递（第') && feed.includes('轮生效）'),
  };
  console.log('D checks:', JSON.stringify(checks), Object.values(checks).every(Boolean) ? 'PASS' : 'FAIL');
  await shot('rg-d-rounds');

  // --- B: detail opens on the task just created (no black screen),
  // ledger/checkpoints/events present
  await page.locator('.card .title').filter({ hasText: title }).first().click();
  await page.waitForTimeout(1500);
  const b = await page.evaluate(() => ({
    headerAlive: !!document.querySelector('header'),
    detail: !!document.querySelector('.detail'),
    ledger: document.querySelector('.detail .ledger')?.innerText?.slice(0, 80) ?? null,
    evCount: document.querySelectorAll('.detail .evlist .ev').length,
    cps: document.querySelectorAll('.detail .cps .cp').length,
  }));
  console.log('B detail:', JSON.stringify(b), (b.headerAlive && b.detail) ? 'PASS' : 'FAIL');
  await shot('rg-b-detail');

  // --- E (U5): result card on the done task: summary + stats + diff expand
  const e0 = await page.evaluate(() => ({
    card: !!document.querySelector('.detail .result-card'),
    state: document.querySelector('.rc-state')?.innerText ?? null,
    summary: document.querySelector('.rc-summary')?.innerText?.slice(0, 60) ?? null,
    stats: document.querySelector('.rc-stats')?.innerText.replace(/\n/g, ' ').slice(0, 120) ?? null,
  }));
  const e1 = await page.getByRole('button', { name: /查看变更明细/ }).click()
    .then(() => page.waitForTimeout(1200))
    .then(() => page.evaluate(() => ({
      meta: document.querySelector('.rc-diff-meta')?.innerText ?? null,
      patchLen: document.querySelector('.rc-patch')?.innerText?.length ?? 0,
      patchHead: document.querySelector('.rc-patch')?.innerText?.slice(0, 80) ?? null,
    })))
    .catch(err => ({ clickFail: String(err).slice(0, 120) }));
  const eChecks = {
    cardShown: e0.card && /完成|✓/.test(e0.state ?? ''),
    statsShown: /验收门/.test(e0.stats ?? '') && /花费/.test(e0.stats ?? ''),
    diffMeta: !!e1.meta && /文件/.test(e1.meta),
    diffPatch: (e1.patchLen ?? 0) > 0,
  };
  console.log('E result-card:', JSON.stringify({ e0, e1 }), Object.values(eChecks).every(Boolean) ? 'PASS' : 'FAIL');
  await shot('rg-e-resultcard');
  await page.evaluate(() => {
    const b2 = Array.from(document.querySelectorAll('button')).find(x => x.textContent.includes('关闭'));
    b2?.click();
  });
  await page.waitForTimeout(400);

  console.log('ERRORS:', errors.join(' | ') || '(none)');
  await browser.close();
})().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
