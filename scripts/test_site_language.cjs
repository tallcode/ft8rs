const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { resolve } = require("node:path");
const { test } = require("node:test");
const { runInNewContext } = require("node:vm");

const script = readFileSync(resolve(__dirname, "../site/language.js"), "utf8");

function visit(navigator, href = "https://ft8.rs/") {
  const current = new URL(href);
  let redirect;
  runInNewContext(script, {
    navigator,
    URL,
    location: {
      href,
      search: current.search,
      hash: current.hash,
      replace: (target) => { redirect = target; },
    },
  });
  return redirect;
}

test("Chinese and English regional preferences select their language", () => {
  assert.equal(visit({ languages: ["zh-CN", "en-US"] }), "https://ft8.rs/zh/");
  assert.equal(visit({ languages: ["zh-TW"] }), "https://ft8.rs/zh/");
  assert.equal(visit({ languages: ["en-GB", "zh-CN"] }), "https://ft8.rs/en/");
});

test("the first supported browser preference wins", () => {
  assert.equal(visit({ languages: ["fr-FR", "zh", "en"] }), "https://ft8.rs/zh/");
  assert.equal(visit({ languages: ["EN-us", "zh"] }), "https://ft8.rs/en/");
});

test("unsupported or unavailable preferences fall back to English", () => {
  assert.equal(visit({ languages: ["ja-JP"] }), "https://ft8.rs/en/");
  assert.equal(visit({}), "https://ft8.rs/en/");
  assert.equal(visit({ languages: [], language: "zh-CN" }), "https://ft8.rs/zh/");
});

test("language redirects preserve links to sections and URL parameters", () => {
  assert.equal(
    visit({ languages: ["zh"] }, "https://ft8.rs/?from=radio#download"),
    "https://ft8.rs/zh/?from=radio#download",
  );
});

test("repository subpaths also work before a custom domain is configured", () => {
  assert.equal(
    visit({ languages: ["en"] }, "https://tallcode.github.io/ft8rs/index.html#usage"),
    "https://tallcode.github.io/ft8rs/en/#usage",
  );
});
