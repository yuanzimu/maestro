const { test } = require("node:test");
const assert = require("node:assert");
const { slugify } = require("./index.js");

test("ascii 基本分词", () => {
  assert.equal(slugify("Hello World"), "hello-world");
});

test("连续空格折叠为单个连字符", () => {
  assert.equal(slugify("a   b"), "a-b");
});

test("首尾空白不产生首尾连字符", () => {
  assert.equal(slugify("  Hi  "), "hi");
});

test("大小写归一", () => {
  assert.equal(slugify("Maestro Bench"), "maestro-bench");
});
