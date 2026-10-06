const { test } = require("node:test");
const assert = require("node:assert");
const { csv_column } = require("./index.js");

const CSV = "name,score,city\nalice,90,SH\nbob,85,BJ\ncarol,77,SZ";

test("抽取存在的列", () => {
  assert.deepEqual(csv_column(CSV, "name"), ["alice", "bob", "carol"]);
  assert.deepEqual(csv_column(CSV, "score"), ["90", "85", "77"]);
});

test("列不存在返回 null", () => {
  assert.equal(csv_column(CSV, "nope"), null);
});

test("末行换行容忍", () => {
  assert.deepEqual(csv_column("a,b\n1,2\n", "b"), ["2"]);
});

test("只有表头时为空数组", () => {
  assert.deepEqual(csv_column("a,b\n", "a"), []);
});
