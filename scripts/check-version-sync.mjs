// 版本一致性门禁：断言版本号在**五个文件、六个落点**上完全一致。
//
// 背景（ADR-0009 D1/D2）：
// 版本变更的唯一入口是 `scripts/bump-version.mjs`，它同步五处版本号。该脚本自身带
// 一致性保护（不一致即 exit(1)），但**没有任何 CI 步骤在平时校验一致性** —— 于是
// 一旦有人绕过脚本手工改 `package.json`，漂移会一直潜伏到下一次发版才爆炸
// （本仓库实测：package-lock.json 停在 0.7.1，其余四处 0.9.0，直接阻断发布）。
//
// 本脚本把"一致性"从人工约定升级为 CI 可判定的门禁：任一落点不同即 exit(1)。
//
// 六落点：package.json · package-lock.json(顶层) · package-lock.json(packages[""]) ·
//         src-tauri/Cargo.toml · src-tauri/tauri.conf.json · src-tauri/Cargo.lock
//
// 用法：node scripts/check-version-sync.mjs
// 退出码：0 = 一致；1 = 漂移（差异已打印）或读取失败。

import { readFileSync } from "node:fs";

/// 从文本中取首个捕获组；未命中返回 null（由调用方判定为漂移）
function capture(content, pattern) {
  const found = content.match(pattern);
  return found ? found[1] : null;
}

/// 一次读取 + 解析（避免同一文件重复 I/O）
function readText(file) {
  return readFileSync(file, "utf8");
}

function readJson(file) {
  return JSON.parse(readText(file));
}

/// 采集六个落点。任一读取/解析失败 → 记 error 并判定失败（不静默跳过）
function collect() {
  const points = [];
  const push = (label, read) => {
    try {
      const version = read();
      points.push({ label, version: version ?? null, error: null });
    } catch (cause) {
      points.push({ label, version: null, error: cause instanceof Error ? cause.message : String(cause) });
    }
  };

  push("package.json", () => readJson("package.json").version);

  // package-lock.json 有两个落点，只读一次
  const lockText = readText("package-lock.json");
  const lock = JSON.parse(lockText);
  push("package-lock.json (root version)", () => lock.version);
  push("package-lock.json (packages[''])", () => lock.packages?.[""]?.version);

  push("src-tauri/Cargo.toml", () => capture(readText("src-tauri/Cargo.toml"), /^version = "([^"]+)"/m));

  push("src-tauri/tauri.conf.json", () =>
    capture(readText("src-tauri/tauri.conf.json"), /"version": "([^"]+)"/),
  );

  // 只匹配 dsh-launcher 包自身的 version 段（行尾兼容 CRLF/LF）
  push("src-tauri/Cargo.lock", () =>
    capture(readText("src-tauri/Cargo.lock"), /name = "dsh-launcher"\r?\nversion = "([^"]+)"/),
  );

  return points;
}

const points = collect();

console.log("版本一致性检查（五文件 / 六落点）：");
for (const point of points) {
  const shown = point.error !== null ? `<读取失败: ${point.error}>` : String(point.version);
  console.log(`  ${point.label.padEnd(34)} = ${shown}`);
}

const broken = points.filter((point) => point.error !== null || point.version === null || point.version === "");
const distinct = new Set(points.filter((point) => point.version !== null).map((point) => point.version));

if (broken.length > 0 || distinct.size !== 1) {
  console.error("");
  console.error("✗ 版本号不同步：五文件六落点必须完全一致。");
  console.error(`  实际出现 ${distinct.size} 个不同版本: ${[...distinct].join(", ") || "(无)"}`);
  if (points.length !== 6 || broken.length > 0) {
    console.error(`  另有 ${points.length - points.filter((p) => p.error === null && p.version !== null).length} 个落点读取失败（见上表）。`);
  }
  console.error("  修复：以 package.json 为权威，先手工对齐漂移项，再运行本检查确认。");
  console.error("  注意：版本递增本身必须经 `node scripts/bump-version.mjs --patch|--minor|--major` 完成。");
  process.exit(1);
}

console.log("");
console.log(`✓ 版本一致: ${[...distinct][0]}`);
process.exit(0);
