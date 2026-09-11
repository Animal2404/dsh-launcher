# CSP 回归验证（仅开发验证用）
#
# 目的（ADR-0009 D6）：把 `src-tauri/tauri.conf.json` 里**实际配置的** `csp` 应用到真实
# 前端产物（`dist/`），在真实 Chromium 中加载，断言「零 CSP 违规」——即策略不误伤自身的
# 脚本/样式/字体/图片/数据 URI。
#
# 为什么必须自动化：CSP 配错的典型症状是**功能静默失效**（样式丢失、toast 不显示），
# 肉眼很难第一时间归因；而策略字符串一旦与产物实际引用的资源形态脱节（例如新增一个
# `data:` 图片或运行时注入 <style>），只有实测才能发现。
#
# 断言口径：
#   - `securitypolicyviolation` 事件（页面内注册，覆盖后续所有资源加载）必须为 0；
#   - 控制台不得出现 "Content Security Policy" 相关报错；
#   - 产物 CSS 与字体必须真的加载成功（非 0 字节响应）。
# 说明：本脚本用 HTTP header 下发 CSP（Tauri 亦以响应头下发，见 tauri 的 `csp_header`），
# 故与运行时同构；`ipc:` / `http://ipc.localhost` 在 Tauri 之外不可用，属预期，不计入违规。
#
# 用法：python scripts/verify-csp.py
# 依赖：playwright（python）——与 scripts/verify-layout.py 同一依赖。
import http.server
import json
import pathlib
import socketserver
import threading

ROOT = pathlib.Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
CONF = ROOT / "src-tauri" / "tauri.conf.json"


def load_csp() -> str:
    """从 tauri.conf.json 读实际配置的 csp（防文档/实现漂移）"""
    with CONF.open(encoding="utf-8") as fh:
        conf = json.load(fh)
    csp = conf["app"]["security"]["csp"]
    if not csp:
        raise SystemExit("✗ tauri.conf.json 的 security.csp 为空 —— D6 尚未生效")
    return csp


class Handler(http.server.SimpleHTTPRequestHandler):
    csp = ""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(DIST), **kwargs)

    def end_headers(self):
        self.send_header("Content-Security-Policy", self.csp)
        super().end_headers()

    def log_message(self, *args):  # 静默访问日志
        pass


def main() -> int:
    csp = load_csp()
    Handler.csp = csp
    print(f"CSP 生效值（来自 tauri.conf.json）：\n  {csp}\n")

    with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
        port = httpd.server_address[1]
        threading.Thread(target=httpd.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{port}"

        from playwright.sync_api import sync_playwright

        violations = []
        console_errors = []
        failed_requests = []
        loaded_bytes = {}

        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1600, "height": 900})
            # 在任何资源加载前注册监听（add_init_script 先于页面脚本执行）
            page.add_init_script(
                """
                window.__cspViolations = [];
                document.addEventListener('securitypolicyviolation', function (e) {
                  window.__cspViolations.push({
                    directive: e.violatedDirective,
                    blocked: e.blockedURI,
                    sample: (e.sample || '').slice(0, 120),
                  });
                });
                """
            )
            page.on("console", lambda m: console_errors.append(m.text) if m.type == "error" else None)
            page.on("requestfailed", lambda r: failed_requests.append(f"{r.url} :: {r.failure}"))
            page.on(
                "response",
                lambda r: loaded_bytes.__setitem__(r.url, len(r.body() or b""))
                if r.ok and any(r.url.endswith(ext) for ext in (".css", ".woff2", ".js"))
                else None,
            )

            page.goto(base, wait_until="load")
            page.wait_for_timeout(2500)  # 留出字体/样式/异步 chunk 的加载窗口
            violations = page.evaluate("() => window.__cspViolations || []")
            body_len = page.evaluate("() => (document.body ? document.body.innerHTML.length : -1)")
            css_href = page.evaluate(
                "() => { const l = document.querySelector('link[rel=stylesheet]'); return l ? l.href : null; }"
            )
            font_ok = page.evaluate(
                "() => document.fonts ? document.fonts.size : -1"
            )
            browser.close()

        httpd.shutdown()

    # ---- 判定 ----
    results = []

    def check(name, ok, detail=""):
        results.append((name, ok))
        print(("PASS" if ok else "FAIL") + "  " + name + (("  ->  " + str(detail)) if detail else ""))

    # Tauri IPC 在 Tauri 之外必然失败，属预期，先剔除。
    # 注意：报错文本常带中文前缀（如「订阅日志事件失败 …」），故不能只匹配英文关键词；
    # `transformCallback` / `__TAURI_INTERNALS__` 是 tauri 前端 API 在非 Tauri 宿主下的
    # 特有失败特征，用它判定最可靠。
    def is_expected_noise(text: str) -> bool:
        low = text.lower()
        return (
            "transformcallback" in low
            or "__tauri_internals__" in low
            or "ipc" in low
            or "tauri" in low
            or "invoke" in low
            or "getcurrent" in low
            or "resizeobserver" in low
        )

    real_violations = [v for v in violations]
    real_console = [c for c in console_errors if not is_expected_noise(c)]

    check("零 CSP 违规（securitypolicyviolation）", len(real_violations) == 0, real_violations[:3])
    check("无 CSP 相关控制台报错", len(real_console) == 0, real_console[:3])
    check("页面已渲染（body 非空）", body_len > 500, f"bodyLen={body_len}")
    check("样式表已挂载", bool(css_href), css_href)
    check("字体已注册（@font-face 生效）", isinstance(font_ok, int) and font_ok > 0, f"fontFaces={font_ok}")

    css_url = next((u for u in loaded_bytes if u.endswith(".css")), None)
    check(
        "CSS 实际加载成功（非 0 字节）",
        bool(css_url) and loaded_bytes[css_url] > 0,
        f"{css_url} => {loaded_bytes.get(css_url, 0)} B" if css_url else "未观测到 CSS 响应",
    )
    woff = [u for u in loaded_bytes if u.endswith(".woff2")]
    check("至少一个字体文件加载成功", len(woff) > 0, f"{len(woff)} 个 woff2")

    failed = [f for f in failed_requests if not is_expected_noise(f)]
    check("无非预期请求失败", len(failed) == 0, failed[:3])

    bad = [n for n, ok in results if not ok]
    print(f"\n===== {len(results) - len(bad)}/{len(results)} 通过 =====")
    if bad:
        print("失败项：" + "、".join(bad))
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main())
