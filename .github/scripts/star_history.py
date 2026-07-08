#!/usr/bin/env python3
"""Generate light/dark star-history SVG charts for a GitHub repository.

Fetches the full stargazer timeline via the GitHub REST API (requires a token
with access to the repo; since June 2026 the stargazers list endpoint is
limited to admins/collaborators, which the Actions GITHUB_TOKEN satisfies for
the repository the workflow runs in) and renders a cumulative step chart as
two self-contained SVGs (light and dark) with no external dependencies.

Env vars: GITHUB_REPOSITORY (owner/repo), GITHUB_TOKEN.
Usage: python3 star_history.py [output_dir]
"""

import json
import math
import os
import sys
import urllib.request
from datetime import datetime, timezone

API = "https://api.github.com"


def api_get(path, token):
    req = urllib.request.Request(
        API + path,
        headers={
            "Accept": "application/vnd.github.star+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "star-history-action",
        },
    )
    with urllib.request.urlopen(req) as resp:
        return json.load(resp)


def fetch_star_times(repo, token):
    """Return sorted list of datetime objects, one per star event."""
    times = []
    page = 1
    while page <= 400:  # API caps stargazer pagination at 400 pages (40k stars)
        batch = api_get(f"/repos/{repo}/stargazers?per_page=100&page={page}", token)
        if not batch:
            break
        for item in batch:
            ts = item.get("starred_at")
            if ts:
                times.append(
                    datetime.strptime(ts, "%Y-%m-%dT%H:%M:%SZ").replace(
                        tzinfo=timezone.utc
                    )
                )
        if len(batch) < 100:
            break
        page += 1
    times.sort()
    return times


def nice_step(max_val, target_ticks=4):
    if max_val <= 0:
        return 1
    raw = max_val / target_ticks
    mag = 10 ** math.floor(math.log10(raw))
    for m in (1, 2, 5, 10):
        if raw <= m * mag:
            return max(1, int(m * mag))
    return max(1, int(10 * mag))


def month_ticks(t0, t1, max_ticks=6):
    """Tick datetimes at month starts across [t0, t1], thinned to max_ticks."""
    ticks = []
    y, m = t0.year, t0.month
    # first month boundary at or after t0
    if not (t0.day == 1 and t0.hour == 0):
        m += 1
        if m > 12:
            m, y = 1, y + 1
    while True:
        t = datetime(y, m, 1, tzinfo=timezone.utc)
        if t > t1:
            break
        ticks.append(t)
        m += 1
        if m > 12:
            m, y = 1, y + 1
    if len(ticks) > max_ticks:
        stride = math.ceil(len(ticks) / max_ticks)
        ticks = ticks[::stride]
    return ticks


MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
          "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def fmt_tick(t, span_days):
    if span_days > 300:
        return f"{MONTHS[t.month - 1]} {t.year}"
    return f"{MONTHS[t.month - 1]} {t.day}" if t.day != 1 else MONTHS[t.month - 1]


THEMES = {
    "light": {
        "series": "#2a78d6",
        "surface": "#ffffff",   # GitHub light page background (for the dot ring)
        "grid": "#e6e5df",
        "baseline": "#c3c2b7",
        "ink": "#0b0b0b",
        "muted": "#898781",
    },
    "dark": {
        "series": "#3987e5",
        "surface": "#0d1117",   # GitHub dark page background
        "grid": "#2c2c2a",
        "baseline": "#383835",
        "ink": "#ffffff",
        "muted": "#898781",
    },
}


def render_svg(times, total, theme, now):
    c = THEMES[theme]
    W, H = 800, 320
    # Right margin holds the endpoint dot + count label; scale with digits.
    label_w = 10 + round(9.5 * len(f"{total:,}"))
    ML, MR, MT, MB = 46, max(44, 16 + label_w), 18, 34
    pw, ph = W - ML - MR, H - MT - MB

    t0 = times[0] if times else now
    t1 = now
    span = max((t1 - t0).total_seconds(), 1.0)
    span_days = span / 86400

    def x(t):
        return ML + pw * ((t - t0).total_seconds() / span)

    ystep = nice_step(max(total, 1))
    ymax = max(ystep * math.ceil(max(total, 1) / ystep), ystep)

    def y(v):
        return MT + ph * (1 - v / ymax)

    # Cumulative step path: horizontal to each star's time, then up one.
    # Past ~2000 events, decimate (evenly in event order, exact counts kept
    # at sampled events) so the SVG stays small.
    events = list(enumerate(times, start=1))  # (cumulative count, time)
    if len(events) > 2000:
        stride = math.ceil(len(events) / 2000)
        events = events[stride - 1::stride]
        if events[-1][0] != len(times):
            events.append((len(times), times[-1]))
    pts = [(x(t0), y(0))]
    for count, t in events:
        px = x(t)
        pts.append((px, pts[-1][1]))
        pts.append((px, y(count)))
    pts.append((x(t1), pts[-1][1] if times else y(0)))

    def fmt(p):
        return f"{p[0]:.1f},{p[1]:.1f}"

    line_d = "M" + " L".join(fmt(p) for p in pts)
    area_d = (
        f"M{ML:.1f},{y(0):.1f} "
        + " L".join(fmt(p) for p in pts)
        + f" L{x(t1):.1f},{y(0):.1f} Z"
    )

    font = 'font-family="system-ui, -apple-system, &quot;Segoe UI&quot;, sans-serif"'
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" '
        f'width="{W}" height="{H}" role="img" '
        f'aria-label="Cumulative GitHub stars over time: {total}">'
    ]

    # Horizontal gridlines + y tick labels (skip 0's gridline; baseline covers it)
    v = ystep
    while v <= ymax:
        gy = y(v)
        parts.append(
            f'<line x1="{ML}" y1="{gy:.1f}" x2="{ML + pw}" y2="{gy:.1f}" '
            f'stroke="{c["grid"]}" stroke-width="1"/>'
        )
        parts.append(
            f'<text x="{ML - 8}" y="{gy + 4:.1f}" text-anchor="end" '
            f'{font} font-size="12" fill="{c["muted"]}" '
            f'style="font-variant-numeric: tabular-nums">{v:,}</text>'
        )
        v += ystep
    parts.append(
        f'<text x="{ML - 8}" y="{y(0) + 4:.1f}" text-anchor="end" '
        f'{font} font-size="12" fill="{c["muted"]}">0</text>'
    )

    # Baseline
    parts.append(
        f'<line x1="{ML}" y1="{y(0):.1f}" x2="{ML + pw}" y2="{y(0):.1f}" '
        f'stroke="{c["baseline"]}" stroke-width="1"/>'
    )

    # X ticks
    for t in month_ticks(t0, t1):
        tx = x(t)
        parts.append(
            f'<line x1="{tx:.1f}" y1="{y(0):.1f}" x2="{tx:.1f}" y2="{y(0) + 5:.1f}" '
            f'stroke="{c["baseline"]}" stroke-width="1"/>'
        )
        parts.append(
            f'<text x="{tx:.1f}" y="{y(0) + 20:.1f}" text-anchor="middle" '
            f'{font} font-size="12" fill="{c["muted"]}">{fmt_tick(t, span_days)}</text>'
        )

    # Area wash (series hue at 10%) and 2px step line
    parts.append(f'<path d="{area_d}" fill="{c["series"]}" fill-opacity="0.1"/>')
    parts.append(
        f'<path d="{line_d}" fill="none" stroke="{c["series"]}" stroke-width="2" '
        f'stroke-linejoin="round" stroke-linecap="round"/>'
    )

    # End marker: 2px surface ring + r=4 dot, endpoint label in primary ink
    ex, ey = x(t1), y(total)
    parts.append(f'<circle cx="{ex:.1f}" cy="{ey:.1f}" r="6" fill="{c["surface"]}"/>')
    parts.append(f'<circle cx="{ex:.1f}" cy="{ey:.1f}" r="4" fill="{c["series"]}"/>')
    parts.append(
        f'<text x="{ex + 10:.1f}" y="{ey + 5:.1f}" text-anchor="start" '
        f'{font} font-size="14" font-weight="600" fill="{c["ink"]}">{total:,}</text>'
    )

    parts.append("</svg>")
    return "".join(parts)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    repo = os.environ["GITHUB_REPOSITORY"]
    token = os.environ["GITHUB_TOKEN"]

    times = fetch_star_times(repo, token)
    # stargazers_count can differ slightly (deleted accounts); the timeline is
    # the chart's source of truth.
    total = len(times)
    now = datetime.now(timezone.utc)

    os.makedirs(out_dir, exist_ok=True)
    for theme in ("light", "dark"):
        path = os.path.join(out_dir, f"star-history-{theme}.svg")
        with open(path, "w", encoding="utf-8") as f:
            f.write(render_svg(times, total, theme, now))
        print(f"wrote {path} ({total} stars)")


if __name__ == "__main__":
    main()
