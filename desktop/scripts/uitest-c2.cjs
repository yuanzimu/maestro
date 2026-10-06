// C2 GUI test: hunk-level partial accept on the result card.
//  1. create a demo task (3 rounds, git workdir) -> wait done
//  2. open detail -> expand diff -> check first hunk + reason
//  3. reject -> assert toast + followup task card appears
// NOTE: ASCII-only. Requires GUI running with CDP :9222 and mock-cli worker
// mode configured (demo). The workdir must be a git repo.
const { chromium } = require('playwright-core');

(async () => {
  const browser = await chromium.connectOverCDP('http://127.0.0.1:9222');
  let page = browser.contexts()[0]?.pages()[0];
  if (!page) throw new Error('no page via CDP');
  for (let i = 0; i < 40 && !page.url().includes('localhost:1420'); i++) {
    await page.waitForTimeout(500).catch(() => {});
  }
  console.log('page-url:', page.url());
  const shot = (n) => page.screenshot({ path: `C:\\dev\\uitest\\${n}.png` });
  const errors = [];
  page.on('pageerror', e => errors.push('PAGEERROR: ' + e.message));
  page.on('console', m => {
    if (m.type() === 'error') errors.push('CONSOLE: ' + m.text().slice(0, 400));
  });

  await page.getByRole('button', { name: /返回任务列表/ }).click().catch(() => {});
  // close a possibly-open detail overlay from a previous session
  await page.evaluate(() => {
    const btn = Array.from(document.querySelectorAll('.detail button'))
      .find(b => b.textContent.includes('关闭'));
    btn?.click();
  });
  await page.waitForTimeout(300);

  // 1. fresh demo task
  const title = 'c2 ' + Date.now().toString().slice(-6);
  await page.getByRole('button', { name: '新建任务' }).click();
  await page.waitForTimeout(300);
  await page.locator('.modal input').first().fill(title);
  await page.locator('.modal textarea').fill('demo: finish in 3 rounds, one section per round');
  await page.locator('.modal input').nth(1).fill('C:\\dev\\maestro-demo');
  await page.getByRole('button', { name: '派活' }).click();

  // wait done (3 rounds x ~1.2s + spawn + margins).
  // NOTE: match the state dot class, NOT innerText "完成" — the narrative
  // "第 N 轮完成：…" of a WORKING card contains 完成 too (false positive).
  let done = false;
  for (let i = 0; i < 60; i++) {
    done = await page.evaluate((t) => {
      const hit = Array.from(document.querySelectorAll('.card'))
        .find(c => c.querySelector('.title')?.textContent?.includes(t));
      return !!hit && !!hit.querySelector('.st.done');
    }, title);
    if (done) break;
    await page.waitForTimeout(1500);
  }
  console.log('task done:', done);
  if (!done) throw new Error('task never reached done state');
  await shot('c2-1-done');

  // 2. open detail — playwright click on the explicit 详情 button
  //    (an earlier hooks-rule bug crashed the detail into ErrorBoundary;
  //    if the stale fatal panel is still up from a previous session of this
  //    GUI, reload the page once to pick up the fixed bundle)
  await page.locator('.card').filter({ hasText: title })
    .getByRole('button', { name: '详情' }).first().click();
  await page.waitForTimeout(1200);
  if (await page.evaluate(() => !!document.querySelector('.fatal'))) {
    console.log('stale fatal panel -> reloading page');
    await page.reload();
    for (let i = 0; i < 40 && !page.url().includes('localhost:1420'); i++) {
      await page.waitForTimeout(500).catch(() => {});
    }
    await page.waitForTimeout(1500);
    await page.locator('.card').filter({ hasText: title })
      .getByRole('button', { name: '详情' }).first().click();
    await page.waitForTimeout(1200);
  }
  const diag = await page.evaluate(() => ({
    detail: !!document.querySelector('.detail'),
    overlay: !!document.querySelector('.overlay'),
    modal: !!document.querySelector('.modal'),
    cards: document.querySelectorAll('.card').length,
  }));
  console.log('diag:', JSON.stringify(diag));
  const kvState = await page.evaluate(() =>
    document.querySelector('.detail .kv')?.innerText.replace(/\n/g, ' ').slice(0, 120) ?? null);
  console.log('detail kv:', kvState);
  // wait up to 5s for the result card (state settle)
  let rcOk = false;
  for (let i = 0; i < 10 && !rcOk; i++) {
    rcOk = await page.evaluate(() => !!document.querySelector('.detail .result-card'));
    if (!rcOk) await page.waitForTimeout(500);
  }
  console.log('result-card shown:', rcOk);
  if (!rcOk) throw new Error('result card not rendered on done task');
  // diff is lazy: click the expand button first
  await page.getByRole('button', { name: /查看变更明细/ }).click();
  await page.waitForTimeout(1500);
  const pre = await page.evaluate(() => ({
    card: !!document.querySelector('.detail .result-card'),
    hunks: document.querySelectorAll('.rc-hunk').length,
    diffMeta: document.querySelector('.rc-diff-meta')?.innerText ?? null,
  }));
  console.log('diff expanded:', JSON.stringify(pre));
  if (pre.hunks === 0) {
    // binary/deleted-only diff: fall back to raw patch view, nothing to reject
    console.log('NO HUNKS (raw patch only) - C2 reject flow not exercisable here');
    await browser.close();
    return;
  }
  await shot('c2-2-diff');

  // 3. check the first hunk, fill reason, submit
  await page.locator('.rc-hunk input[type=checkbox]').first().check();
  await page.locator('.rc-reason').fill('c2-test: this section is wrong, redo it');
  const btnText = await page.locator('.rc-reject-btn').innerText();
  console.log('reject-btn:', btnText);
  await page.locator('.rc-reject-btn').click();
  await page.waitForTimeout(2500);

  const post = await page.evaluate(() => ({
    toast: document.querySelector('.toast')?.textContent ?? null,
    hunksAfter: document.querySelectorAll('.rc-hunk').length,
  }));
  console.log('after reject:', JSON.stringify(post));
  await shot('c2-3-rejected');

  // 4. close detail, followup task card must exist
  await page.evaluate(() => {
    const b2 = Array.from(document.querySelectorAll('button')).find(x => x.textContent.includes('关闭'));
    b2?.click();
  });
  await page.waitForTimeout(800);
  const cards = await page.evaluate(() =>
    Array.from(document.querySelectorAll('.card .title')).map(t => t.textContent));
  const followup = cards.some(c => c && c.includes('修正:'));
  console.log('followup card:', followup, JSON.stringify(cards));

  const checks = {
    toastOk: !!post.toast && post.toast.includes('已拒绝 1 段'),
    followup,
    errors: errors.length === 0,
  };
  console.log('C2 checks:', JSON.stringify(checks), Object.values(checks).every(Boolean) ? 'PASS' : 'FAIL');
  console.log('ERRORS:', errors.join(' | ') || '(none)');
  await browser.close();
})().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
