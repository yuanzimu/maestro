const { test } = require("node:test");
const assert = require("node:assert");
const { is_email, is_phone, validate } = require("./index.js");

test("统一入口：email", () => {
  assert.equal(validate("user@example.com", "email"), true);
  assert.equal(validate("  user@example.com  ", "email"), true);
  assert.equal(validate("no-at-sign", "email"), false);
});

test("统一入口：phone", () => {
  assert.equal(validate("+8613800138000", "phone"), true);
  assert.equal(validate(" 13800138000 ", "phone"), true);
  assert.equal(validate("123", "phone"), false);
});

test("统一入口：未知 kind 抛错", () => {
  assert.throws(() => validate("x", "unknown"));
});

test("旧 API 行为保持", () => {
  assert.equal(is_email("a@b.co"), true);
  assert.equal(is_phone("+12345678901"), true);
});
