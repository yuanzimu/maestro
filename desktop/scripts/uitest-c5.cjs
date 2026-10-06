// C5 GUI test: U7 feedback loop on the result card.
//  1. demo task A -> done -> detail -> idle feedback UI
//  2. negative flow: click 不好 -> reason required (submit disabled when
//     empty) -> fill + submit -> toast + already-feedback state + memory entry
//  3. demo task B -> done -> detail -> positive flow: 👍 direct submit ->
//     toast + state + memory entry (no remark)
//  4. close/reopen detail -> feedback state restored from the event stream
// NOTE: run with the GUI on CDP :9222 and the daemon sidecar rebuilt with
// task.feedback support (prepare-sidecars.ps1). Workdir must be a git repo.
const { chromium } = require('playwright-core');
const fs = require('fs');

const WORKDIR = 'C:\\dev\\maestro-demo';
const MEMORY = WORKDIR + '\\MAESTRO_MEMORY.md';

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
  await page.evaluate(() => {
    const btn = Array.from(document.querySelectorAll('.detail button'))
      .find(b => b.textContent.includes('关闭'));
    btn?.click();
  });
  await page.waitForTimeout(300);

  // create a demo task, wait done, open detail, wait result card
  const mkTask = async (title) => {
    await page.getByRole('button', { name: '新建任务' }).click();
    await page.waitForTimeout(300);
    await page.locator('.modal input').first().fill(title);
    await page.locator('.modal textarea').fill('demo: finish in 3 rounds, one section per round');
    await page.locator('.modal input').nth(1).fill(WORKDIR);
    await page.getByRole('button', { name: '派活' }).click();
    // state-dot class, not innerText (narrative of a WORKING card contains 完成)
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
    if (!done) throw new Error('task never reached done: ' + title);
    await page.locator('.card').filter({ hasText: title })
      .getByRole('button', { name: '详情' }).first().click();
    await page.waitForTimeout(1200);
    // stale fatal panel from a previous bundle -> reload once, reopen
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
    let rcOk = false;
    for (let i = 0; i < 10 && !rcOk; i++) {
      rcOk = await page.evaluate(() => !!document.querySelector('.detail .result-card'));
      if (!rcOk) await page.waitForTimeout(500);
    }
    if (!rcOk) throw new Error('result card not rendered: ' + title);
  };
  const closeDetail = async () => {
    await page.evaluate(() => {
      const b = Array.from(document.querySelectorAll('button')).find(x => x.textContent.includes('关闭'));
      b?.click();
    });
    await page.waitForTimeout(800);
  };

  const memBefore = fs.existsSync(MEMORY) ? fs.readFileSync(MEMORY, 'utf8') : '';

  // ---- task A: negative feedback flow ----
  const titleA = 'c5neg ' + Date.now().toString().slice(-6);
  await mkTask(titleA);
  const idle = await page.evaluate(() => {
    const fb = document.querySelector('.detail .rc-feedback');
    return fb ? {
      ask: fb.textContent.includes('这个结果如何'),
      good: !!fb.querySelector('.rc-fb-good'),
      bad: Array.from(fb.querySelectorAll('button')).some(b => b.textContent.includes('不好')),
    } : null;
  });
  console.log('fb idle:', JSON.stringify(idle));
  await shot('c5-1-idle');

  // click 不好 -> reason textarea shows, submit disabled while empty
  await page.locator('.detail .rc-feedback button').filter({ hasText: '不好' }).first().click();
  await page.waitForTimeout(300);
  const negUi = await page.evaluate(() => {
    const fb = document.querySelector('.detail .rc-feedback');
    const ta = fb?.querySelector('textarea');
    const submit = Array.from(fb?.querySelectorAll('button') || [])
      .find(b => b.textContent.includes('提交'));
    return { ta: !!ta, submitDisabled: submit?.disabled ?? null };
  });
  console.log('fb negative ui:', JSON.stringify(negUi));

  const reasonA = 'c5-test: section 2 landed in the wrong file';
  await page.locator('.detail .rc-feedback textarea').fill(reasonA);
  await page.locator('.detail .rc-feedback button').filter({ hasText: '提交' }).first().click();
  await page.waitForTimeout(1800);
  const afterNeg = await page.evaluate(() => ({
    toast: document.querySelector('.toast')?.textContent ?? null,
    fb: document.querySelector('.detail .rc-feedback')?.innerText.replace(/\n/g, ' ').slice(0, 140) ?? null,
  }));
  console.log('after negative:', JSON.stringify(afterNeg));
  await shot('c5-2-neg-done');
  await closeDetail();

  // ---- task B: positive feedback flow ----
  const titleB = 'c5pos ' + Date.now().toString().slice(-6);
  await mkTask(titleB);
  await page.locator('.detail .rc-feedback .rc-fb-good').click();
  await page.waitForTimeout(1800);
  const afterPos = await page.evaluate(() => ({
    toast: document.querySelector('.toast')?.textContent ?? null,
    fb: document.querySelector('.detail .rc-feedback')?.innerText.replace(/\n/g, ' ').slice(0, 140) ?? null,
  }));
  console.log('after positive:', JSON.stringify(afterPos));
  await shot('c5-3-pos-done');

  // ---- reopen: feedback state restored from the event stream ----
  await closeDetail();
  await page.locator('.card').filter({ hasText: titleB })
    .getByRole('button', { name: '详情' }).first().click();
  await page.waitForTimeout(1200);
  const restored = await page.evaluate(() =>
    document.querySelector('.detail .rc-feedback')?.innerText.replace(/\n/g, ' ').slice(0, 140) ?? null);
  console.log('restored after reopen:', restored);

  // ---- MAESTRO_MEMORY.md assertions (daemon appends, append-only) ----
  const memAfter = fs.existsSync(MEMORY) ? fs.readFileSync(MEMORY, 'utf8') : '';
  const added = memAfter.slice(memBefore.length);
  const negInMem = added.includes('[👎]') && added.includes(titleA) && added.includes(reasonA);
  const posInMem = added.includes('[👍]') && added.includes(titleB) && added.includes('（无备注）');

  const checks = {
    idleAsk: !!idle && idle.ask && idle.good && idle.bad,
    negUiOk: !!negUi.ta && negUi.submitDisabled === true,
    negToast: !!afterNeg.toast && afterNeg.toast.includes('已记录 👎'),
    negState: !!afterNeg.fb && afterNeg.fb.includes('已反馈 👎') && afterNeg.fb.includes(reasonA.slice(0, 20)),
    posToast: !!afterPos.toast && afterPos.toast.includes('已记录 👍'),
    posState: !!afterPos.fb && afterPos.fb.includes('已反馈 👍'),
    restoredState: !!restored && restored.includes('已反馈 👍'),
    negInMem, posInMem,
    errors: errors.length === 0,
  };
  console.log('C5 checks:', JSON.stringify(checks), Object.values(checks).every(Boolean) ? 'PASS' : 'FAIL');
  console.log('ERRORS:', errors.join(' | ') || '(none)');
  console.log('--- memory delta ---');
  console.log(added || '(empty)');
  await browser.close();
})().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
