# Aquanter Surface

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Made with iced](https://iced.rs/badge.svg)](https://github.com/iced-rs/iced)

A native desktop order-flow charting and analysis platform for crypto markets, built on top of
[Flowsurface](https://github.com/flowsurface-rs/flowsurface). Supports Binance, Bybit, Hyperliquid, OKX, and MEXC.

> **Unofficial fork.** Aquanter Surface is a modified version of
> [flowsurface-rs/flowsurface](https://github.com/flowsurface-rs/flowsurface), maintained independently by Aquanter.
> It is not affiliated with or endorsed by the Flowsurface project. Please report issues here, not upstream.

> **Disclaimer.** For research and education only. Nothing in this software is investment advice.
> Aquanter does not provide brokerage, custody, order matching, or trading services. Live trading
> features act only on your own exchange account with your own API keys, at your own risk.

<div align="center">
  <img
    src="assets/screenshot.png"
    alt="Aquanter Surface screenshot"
    style="max-width: 100%; height: auto;"
  />
</div>

## What's Different from Upstream

Everything below is added in this fork. All upstream features remain available.

### AI market analysis
- Builds a market snapshot from the active Kline/Footprint pane (price, order flow, open interest) and asks an LLM for a directional read.
- Providers: OpenAI, OpenRouter, DeepSeek, xAI, Groq, Mistral, Anthropic, Gemini, or any OpenAI-compatible endpoint. Keys are stored in the OS keychain.
- Scores directional setups with confidence and signal validity, and keeps a history that records the outcomes and can be filtered by confidence.
- Can run automatically when an alert fires.

### Alerts
- Rules for price touches, volume change, and bar close.
- Each rule can notify in the app, send a system notification, and/or trigger an AI analysis.

### Trading panel
- Order entry with leverage, margin mode, and position management, drawn as lines on the chart.
- Three modes: **Binance Testnet** (default), **Local Simulation**, and **Binance Live**. You can require confirmation before each order.
- **Event contract simulator:** simulated binary up/down contracts that are opened and settled locally.

### Order-flow tools
- **Cross-venue aggregation:** merge the same coin's order flow from several exchanges into one chart. Pick the sources in the ticker selector, with optional basis correction between spot and perps.
- **Footprint:** automatic tick size, larger tick multipliers, per-row delta when bid/ask text won't fit, and a docked summary.
- **Heatmap:** markers for pulled and absorbed liquidity walls, plus a delta mode for the volume profile.
- **Kline:** crosshair sync across panes, latest price kept away from the chart edge, AVWAP anchored by default, and drawn volume profiles with a row-size control.

### Extra bundled indicators
`pro-cvd`, `pro-open-interest`, `oi-delta-divergence`, `oi-flow-net`, `oi-weighted-delta`, `order-flow-bias`, `liquidation-pressure`.

### Other
- Any IANA time zone can be chosen, not only UTC and Local.
- Fixes for unpublished Binance archives, starved perp depth, and an hourly trade cache.

### Compatibility notes
- Settings, credentials, and data stay in the same `flowsurface/` data directory, so an existing upstream install can be opened directly.
- Indicators from the upstream catalog still work. The catalog is fetched from upstream's `api.flowsurface.com`.

## Key Features

- Multiple chart/pane types:
    - **Time & Sales:** Scrollable list of live trades that can be merged across multiple tickers.
    - **DOM (Depth of Market) / Ladder:** Displays current orderbook alongside recent trade volumes on grouped price levels.
    - **Heatmap (Historical DOM):** Uses live trades and orderbook to display a time-series heatmap chart. Supports customizable price grouping and different timeframes.
    - **Candlestick:** Footprints, profiles, scriptable indicators, custom tick-based intervals. See [Indicators](#indicators) and [Studies](#studies).
- Real-time sound effects driven by trade streams
- Multi window/monitor support on macOS and Linux.[\*](#platform-notes)
- Persistent layouts, chart presets, themes with editable color palettes.

## Market Data

**Live:** Trades, orderbook, and kline updates stream from exchange WebSocket feeds.

**Historical trades:** Optional backfill for the visible range on time-based candlestick charts.
Can be enabled in **Settings → Network**. Three modes are available:

- **Off**: No historical backfill.
- **Exchange**: Fetches directly from Binance's public REST API and bulk data
  mirrors ([data.binance.vision](https://data.binance.vision/)). Daily archives use
  bulk files and are cached on disk for reuse; intraday ranges are paginated and
  rate-limited. **Binance only** (spot, linear, inverse).
- **Server**: Fetches from a server that exposes a
  `GET /trades.arrow` endpoint. The reference implementation, [`flowsurface-server`](https://github.com/flowsurface-rs/server), is a
  self-contained collector that writes trades to DuckDB and serves them over this protocol.
  See its [README](https://github.com/flowsurface-rs/server#get-tradesarrow) for the schema,
  query parameters, and auth. Works with every exchange the app supports, provided the server collects that venue.

**Historical klines, open interest, ticker metadata/stats:** Always fetched from exchange
REST APIs, regardless of the trade-fetch mode.

## Indicators

Candlestick charts include [bundled indicators](data/indicators/README.md), more can be added manually or using the
catalog browser.

**Catalog browser**: Open the chart pane's **Indicators** sidebar and click **+**. The browser fetches
a manifest of contributed indicators from `https://api.flowsurface.com`. The manifest is
built from the [catalog repository](https://github.com/flowsurface-rs/indicators/tree/main/sources) at a
pinned commit; installing downloads the selected `.ind` source from the catalog server, verifies it
against the manifest checksum, and compiles it locally before installation.

**Manual install**: Open **Settings → Open data folder** and copy `.ind` files into `indicators/`, then
restart the app. The data directory is `$FLOWSURFACE_DATA_PATH` when set, otherwise your platform's data
directory under `flowsurface/`. The installed-indicator directory can also be overridden with
`FLOWSURFACE_INDICATORS_PATH`.

To write your own indicators, see the [language reference and examples](indicator-engine/LANGUAGE.md) and the
[CLI workflow](indicator-engine/README.md#cli).

## Studies

Footprint requires **Trade data** to be enabled in the pane settings and enough zoom for their columns to be
visible. Backfill is optional and is configured in **Settings → Network**. Some details that may not be clear in the UI:

- A pane uses one profile source: **_candle-based_** or **_trade-based_**. Same-source studies can coexist (one visible-range profile, one recurring profile, one TPO); the `Big Trades` study is trade-only and independent.
- **_Candle-based_** studies are sourced using lower timeframe `Kline` data: `1m` up to `2h`, `3m` up to `4h`, `5m` up to `12h`, and `15m` above that. They are approximations: a candle only gives a high/low range, so volume is spread across that range and TPO marks it. Without a buy/sell split, the candle's volume is treated as buy when `close >= open`, else sell.
- **_Trade-based_** studies use actual prints, so they are more precise, but need trades data, optionally backfill and more processing. Trade versions exist for volume and TPO profiles.
- `TPO` anchors are UTC; initial balance is the first hour from the profile start (for weekly/monthly, the first hour of that week/month).
- The `Big Trades` study filters on quote-currency notional; rings group multiple trades.

## Installation

Prebuilt binaries are not published yet. Build from source:

#### Requirements

- [Rust toolchain](https://www.rust-lang.org/tools/install)
- [Git version control system](https://git-scm.com/)
- System dependencies:
    - **Linux**:
        - Debian/Ubuntu: `sudo apt install build-essential pkg-config libasound2-dev`
        - Arch: `sudo pacman -S base-devel alsa-lib`
        - Fedora: `sudo dnf install gcc make alsa-lib-devel`
    - **macOS**: Install Xcode Command Line Tools: `xcode-select --install`
    - **Windows**: No additional dependencies required

#### Build and run

```bash
# Clone this repository, then from its root:

# Build and run
cargo build --release
cargo run --release
```

### Platform Notes

<details open>
<summary><strong>Windows</strong></summary>

> ⚠️ Multi-window support is disabled on Windows due to a [redraw issue](https://github.com/rust-windowing/winit/issues/3648) affecting iced’s current winit backend.

</details>

## Acknowledgments

- Upstream: [Flowsurface](https://github.com/flowsurface-rs/flowsurface) by akenshaw and contributors, which this project is based on
- Upstream Windows code signing is provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/)
- [Kraken Desktop](https://www.kraken.com/desktop) (formerly [Cryptowatch](https://blog.kraken.com/product/cryptowatch-to-sunset-kraken-pro-to-integrate-cryptowatch-features)), the main inspiration that sparked this project
- [Halloy](https://github.com/squidowl/halloy), an excellent open-source IRC client, the main reference for the foundational code design
- And of course, [iced](https://github.com/iced-rs/iced), the GUI library that makes this possible

## Privacy

Aquanter Surface has no telemetry or analytics. The AI, trading, and proxy features only contact the providers you configure. See [PRIVACY.md](PRIVACY.md) for the services it
contacts and what stays local.

## License

Aquanter Surface is a modified version of Flowsurface and, like upstream, is released under [GPL-3.0-or-later](LICENSE).
Some components use separate licenses:

| Component                                                                               | License                                                                      |
| --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Application, data crate, and [bundled indicators](data/indicators/README.md)            | [GPL-3.0-or-later](LICENSE)                                                  |
| Indicator engine and CLI                                                                | [Apache-2.0](indicator-engine/LICENSE)                                       |
| [Indicator catalog](https://github.com/flowsurface-rs/indicators) and its documentation | [Apache-2.0](https://github.com/flowsurface-rs/indicators/blob/main/LICENSE) |
| Exchange adapters (`flowsurface-exchange`)                                              | [MIT](exchange/LICENSE)                                                      |

Contributions follow the license of the component being changed; submit catalog indicators to the [indicators repository](https://github.com/flowsurface-rs/indicators/blob/main/CONTRIBUTING.md).
