# OpenDTU Display ESP32

A Rust-based monitor for [OpenDTU](https://github.com/tbnobody/OpenDTU) and a
**Zendure 2400AC** home battery, running on the **ESP32-2432S028** ("Cheap Yellow
Display" / CYD). It fetches live data from both devices over WiFi and renders it on
the built-in ILI9341 TFT display.

## Hardware

| Item | Detail |
|------|--------|
| Board | ESP32-2432S028 (CYD) |
| Display | ILI9341 320×240 TFT (SPI) |
| Chip | ESP32 rev 3.1, 240 MHz, 4 MB Flash |

### CYD Display Pinout

| Signal | GPIO |
|--------|------|
| MOSI | 13 |
| MISO | 12 |
| SCLK | 14 |
| CS | 15 |
| DC | 2 |
| Backlight | 21 |

## Prerequisites

- Rust with the `xtensa-esp32-espidf` target (via [esp-rs](https://github.com/esp-rs/esp-idf-template))
- `espflash` CLI (`cargo install espflash`)
- OpenDTU running and reachable on your local network
- Zendure 2400AC reachable on your local network (local HTTP API enabled)

## Configuration

Edit the constants at the top of `src/main.rs`:

```rust
const WIFI_SSID:   &str = "your-wifi-ssid";
const WIFI_PASS:   &str = "your-wifi-password";
const OPENDTU_URL: &str = "http://<opendtu-ip>/api/livedata/status";
const ZENDURE_URL: &str = "http://<zendure-ip>/properties/report";
```

## Build and Flash

```bash
cargo build --release
espflash flash --port /dev/ttyUSB0 --no-stub --monitor target/xtensa-esp32-espidf/release/opendtu-display-esp32
```

> **Important:** The `--no-stub` flag is required — flashing will time out without it.
> The `--monitor` flag keeps the serial monitor open after flashing so you can see logs.

### Troubleshooting: timeout when connecting

If you get `espflash::timeout`, the board may not have entered bootloader mode
automatically. Trigger it manually:

1. Hold the **BOOT** button on the board
2. Press and release **EN/RST** (reset)
3. Release **BOOT**
4. Run the flash command immediately

## Screen Layout

```
┌──────────────────────────────────────┐
│ OpenDTU Monitor                      │  title bar
├──────────────────────────────────────┤
│           Total Power                │
│             1234 W                   │  large green text, centred
├──────────────────────────────────────┤
│ [████████████████░░░░░░░░░░░░░░░░░]  │  battery bar (green/orange/red)
│       56%              CHG  309 W    │  large text: % left, power right
├──────────────────────────────────────┤  CHG = green / DCH = orange / IDLE = dim
│ Inverter Name                    ON  │
│   Power  : 1234.0 W                  │
│   Voltage: 230.0 V                   │
│   Current: 5.365 A                   │
│   Today  : 3 Wh                      │
│   Total  : 1.234 kWh                 │
├──────────────────────────────────────┤
│ Day: 3 Wh           Tot: 1234.0 kWh  │  footer
└──────────────────────────────────────┘
```

Data refreshes every 10 seconds. If Zendure cannot be reached the battery row shows
"Zendure: no data" in red and OpenDTU data continues to display normally.

## Display Notes

The ILI9341 on the CYD uses **BGR** colour order and requires a horizontal flip.
The display is initialised in `src/main.rs` as:

```rust
Builder::new(ILI9341Rgb565, di)
    .orientation(Orientation::new().rotate(Rotation::Deg90).flip_horizontal())
    .color_order(ColorOrder::Bgr)
    .init(&mut delay)
    .unwrap();
```

Without `ColorOrder::Bgr` colours appear inverted; without `flip_horizontal()` text
and graphics appear mirrored.

## Zendure 2400AC Local API

The Zendure is queried via its local REST API — no cloud connection required:

```
GET http://<zendure-ip>/properties/report
```

No authentication is needed. The relevant fields from the response are nested under
`properties`:

| JSON field | Meaning |
|---|---|
| `electricLevel` | Overall battery state of charge (0–100 %) |
| `packInputPower` | Power the battery feeds **into** the system → **discharging** (W) |
| `outputPackPower` | Power the system sends **into** the battery → **charging** (W) |
| `outputHomePower` | Power currently delivered to home loads (W) |

> **Note:** The field names are counterintuitive. `packInputPower` is discharging
> (battery → system) and `outputPackPower` is charging (system → battery).

To enable the local API on the Zendure, add HEMS to the device configuration in the
Zendure app, then exit to apply.

## Character Encoding / UTF-8

The built-in `MonoFont` glyphs from `embedded-graphics` only cover the **ASCII** range.
Characters such as German umlauts (`Ü`, `Ä`, `Ö`), accented letters (`é`, `ñ`), or any
other non-ASCII UTF-8 codepoint will **not render correctly** on the display.

A `to_ascii()` helper in `src/main.rs` automatically transliterates common non-ASCII
characters to their ASCII equivalents before drawing (e.g. `ü → U`, `ß → s`).
Inverter names are passed through this function automatically.

To extend the character map, edit `to_ascii()` in `src/main.rs`:

```rust
fn to_ascii(s: &str) -> String {
    s.chars().map(|c| match c {
        'Ä' | 'ä' => 'A',
        'Ö' | 'ö' => 'O',
        'Ü' | 'ü' => 'U',
        'ß'       => 's',
        // add more mappings here...
        c if c.is_ascii() => c,
        _         => '?',
    }).collect()
}
```

## License

MIT
