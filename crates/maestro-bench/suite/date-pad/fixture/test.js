const { test } = require("node:test");
const assert = require("node:assert");
const { format_date } = require("./index.js");

test("月日补零", () => {
  assert.equal(format_date(new Date(2026, 2, 5)), "2026-03-05");
});

test("两位月日不变形", () => {
  assert.equal(format_date(new Date(2026, 11, 25)), "2026-12-25");
});

test("年份四位", () => {
  assert.equal(format_date(new Date(1999, 0, 1)), "1999-01-01");
});
