import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { test } from "node:test";
import { compileModule } from "svelte/compiler";

registerHooks({
  load(url, context, nextLoad) {
    if (!url.endsWith("/preferences.svelte.ts")) return nextLoad(url, context);
    return {
      format: "module",
      source: compileModule(stripTypeScriptTypes(readFileSync(new URL(url), "utf8")), {
        filename: new URL(url).pathname,
        generate: "client",
      }).js.code,
      shortCircuit: true,
    };
  },
});

let saved = null;
let blocked = false;
globalThis.document = { documentElement: { dataset: {} } };
globalThis.localStorage = {
  getItem: () => saved,
  setItem: (_key, value) => {
    if (blocked) throw new Error("Storage unavailable");
    saved = value;
  },
};

const { preferences } = await import("./preferences.svelte.ts");
const Preferences = preferences.constructor;

test("defaults start in Focus preset with hooks on errors-only and sumi palette", () => {
  saved = null;
  const owner = new Preferences();
  owner.apply();
  assert.equal(owner.hooks, "errors");
  assert.equal(owner.thoughts, "collapsed");
  assert.equal(owner.timestamps, "hover");
  assert.equal(owner.telemetry, "compact");
  assert.equal(owner.palette, "sumi");
  assert.equal(owner.preset, "focus");
  assert.equal(document.documentElement.dataset.palette, "sumi");
});

test("invalid or corrupt saved payloads fall back cleanly to defaults", () => {
  saved = "{bad json";
  let owner = new Preferences();
  assert.equal(owner.hooks, "errors");
  assert.equal(owner.palette, "sumi");

  saved = JSON.stringify({ hooks: "bogus", palette: "neon", timestamps: "auto" });
  owner = new Preferences();
  assert.equal(owner.hooks, "errors");
  assert.equal(owner.palette, "sumi");
  assert.equal(owner.timestamps, "auto");
  assert.equal(owner.preset, "custom");
});

test("presets, toggles, and palette updates persist and restore", () => {
  saved = null;
  const owner = new Preferences();
  owner.applyPreset("debug");
  assert.equal(owner.preset, "debug");
  assert.equal(owner.hooks, "all");
  assert.equal(owner.timestamps, "auto");
  assert.equal(owner.telemetry, "detailed");

  owner.setPalette("violet");
  assert.equal(document.documentElement.dataset.palette, "violet");

  const restored = new Preferences();
  assert.equal(restored.preset, "debug");
  assert.equal(restored.palette, "violet");

  restored.toggleDebug();
  assert.equal(restored.preset, "focus");
  assert.equal(restored.hooks, "errors");

  restored.toggleHooks();
  assert.equal(restored.hooks, "all");
  restored.toggleHooks();
  assert.equal(restored.hooks, "errors");
});

test("storage failure still applies changes in memory and clears once writable", () => {
  blocked = true;
  preferences.setPalette("mono");
  assert.equal(document.documentElement.dataset.palette, "mono");
  assert.match(preferences.storageError, /could not be saved/);

  blocked = false;
  preferences.setPalette("sumi");
  assert.equal(document.documentElement.dataset.palette, "sumi");
  assert.equal(preferences.storageError, undefined);
});
