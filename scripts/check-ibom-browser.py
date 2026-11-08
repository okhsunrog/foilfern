# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "playwright>=1.64.1",
# ]
# ///
"""Exercise an exported CAN–USB assembly BOM offline in headless Chromium.

Writes desktop/compact light/dark screenshots and a JSON result. Optionally
compares an official CLI export, excluding only its generation timestamp.
"""

import argparse
import json
import re
import shutil
from pathlib import Path

from playwright.sync_api import sync_playwright

parser = argparse.ArgumentParser()
parser.add_argument("html", type=Path)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--reference", type=Path)
parser.add_argument(
    "--browser",
    default=shutil.which("google-chrome")
    or shutil.which("chromium")
    or shutil.which("chromium-browser"),
)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
errors = []
requests = []
with sync_playwright() as p:
    browser = p.chromium.launch(
        executable_path=args.browser, headless=True, args=["--disable-dev-shm-usage"]
    )
    context = browser.new_context(viewport={"width": 1440, "height": 960}, offline=True)
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.on(
        "request",
        lambda request: (
            requests.append(request.url)
            if request.url.startswith(("https:", "http:"))
            else None
        ),
    )
    page.goto(args.html.resolve().as_uri())
    comparison = None
    if args.reference:
        actual_html = args.html.read_text()
        reference_html = args.reference.read_text()

        def decode(text):
            encoded = re.search(
                r'var pcbdata = JSON.parse\(LZString.decompressFromBase64\("([^\"]+)"',
                text,
            ).group(1)
            data = page.evaluate(
                "s => JSON.parse(LZString.decompressFromBase64(s))", encoded
            )
            data["metadata"]["date"] = "generation time"
            return encoded, data

        a_encoded, actual = decode(actual_html)
        e_encoded, expected = decode(reference_html)
        for name, data in [("actual", actual), ("reference", expected)]:
            (args.output / f"{name}-data.json").write_text(
                json.dumps(data, indent=2) + "\n"
            )
        assert actual == expected, (
            "PCB data differs; inspect actual-data.json and reference-data.json"
        )
        assert actual_html.replace(a_encoded, "__pcbdata__") == reference_html.replace(
            e_encoded, "__pcbdata__"
        ), "HTML/config differs"
        comparison = "data and HTML/config match after ignoring generation time"
    page.wait_for_selector("#bombody tr")
    initial = page.locator("#bombody tr").count()
    assert initial > 0
    row = page.locator("#bombody tr").first
    row.locator("td").last.click()
    page.wait_for_function("highlightedFootprints.length > 0")
    selected = page.evaluate("highlightedFootprints")
    assert selected
    checkbox = row.locator('input[type="checkbox"]').last
    checkbox.check()
    page.reload()
    page.wait_for_selector("#bombody tr")
    assert (
        page.locator("#bombody tr")
        .first.locator('input[type="checkbox"]')
        .last.is_checked()
    )
    reference = page.evaluate("pcbdata.bom.F[0][0][0]")
    page.locator("#filter").fill(reference)
    assert 0 < page.locator("#bombody tr").count() < initial
    page.locator("#filter").fill("")
    page.locator("#fl-btn").click()
    page.locator("#bombody tr").first.locator("td").last.click()
    page.screenshot(
        path=str(args.output / "desktop-light.png"),
        full_page=True,
        animations="disabled",
    )
    page.locator(".menubtn").hover()
    page.locator("#darkmodeCheckbox").check()
    page.locator("#title").hover()
    page.screenshot(
        path=str(args.output / "desktop-dark.png"),
        full_page=True,
        animations="disabled",
    )
    page.locator("#bl-btn").click()
    page.screenshot(
        path=str(args.output / "back.png"), full_page=True, animations="disabled"
    )
    page.locator("#fb-btn").click()
    page.set_viewport_size({"width": 390, "height": 844})
    page.locator("#tb-btn").click()
    page.screenshot(
        path=str(args.output / "compact-dark.png"),
        full_page=True,
        animations="disabled",
    )
    page.locator(".menubtn").hover()
    page.locator("#darkmodeCheckbox").uncheck()
    page.locator("#title").hover()
    page.screenshot(
        path=str(args.output / "compact-light.png"),
        full_page=True,
        animations="disabled",
    )
    report = {
        "offline": True,
        "rows": initial,
        "highlighted": selected,
        "checkbox_persisted": True,
        "errors": errors,
        "network_requests": requests,
        "metadata": page.evaluate("pcbdata.metadata"),
    }
    report["oracle_comparison"] = comparison
    (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    assert not errors, errors
    assert not requests, requests
    browser.close()
print(json.dumps(report, indent=2))
