use std::thread;
use std::time::Duration;
// ESP-IDF HAL
use esp_idf_hal::delay::Ets;
use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::spi::{config::Config as SpiConfig, SpiDeviceDriver, SpiDriver, SpiDriverConfig};
use esp_idf_hal::units::FromValueType;
// ESP-IDF SVC
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};
// Embedded traits
use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use embedded_svc::wifi::AuthMethod;
// Display
use display_interface_spi::SPIInterface;
use u8g2_fonts::{fonts, FontRenderer};
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyleBuilder, Rectangle},
};
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use mipidsi::Builder;
// JSON
use serde::Deserialize;
// ── Configuration ─────────────────────────────────────────────────────────────
const WIFI_SSID:   &str = "WifiName";
const WIFI_PASS:   &str = "WiFiPassword";
const OPENDTU_URL: &str = "http://192.168.1.40/api/livedata/status";
const ZENDURE_URL: &str = "http://192.168.11.154/properties/report";
// ── CYD (ESP32-2432S028) display pinout ───────────────────────────────────────
// MOSI: GPIO13  MISO: GPIO12  SCLK: GPIO14
// CS:   GPIO15  DC:   GPIO2   Backlight: GPIO21
// ── OpenDTU JSON structs ───────────────────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct LiveData {
    inverters: Vec<Inverter>,
    total: TotalAC,
}
#[derive(Debug, Deserialize)]
struct Inverter {
    serial: String,
    name: String,
    reachable: bool,
    producing: bool,
    #[serde(rename = "AC")]
    ac: Option<AcData>,
    #[serde(rename = "DC")]
    dc: Option<DcData>,
}
#[derive(Debug, Deserialize)]
struct AcData {
    #[serde(rename = "0")]
    phase0: Option<AcPhase>,
}
#[derive(Debug, Deserialize)]
struct AcPhase {
    #[serde(rename = "Power")]
    power: Option<ValueUnit>,
    #[serde(rename = "Voltage")]
    voltage: Option<ValueUnit>,
    #[serde(rename = "Current")]
    current: Option<ValueUnit>,
}
#[derive(Debug, Deserialize)]
struct DcData {
    #[serde(rename = "0")]
    string0: Option<DcString>,
}
#[derive(Debug, Deserialize)]
struct DcString {
    #[serde(rename = "YieldDay")]
    yield_day: Option<ValueUnit>,
    #[serde(rename = "YieldTotal")]
    yield_total: Option<ValueUnit>,
}
#[derive(Debug, Deserialize)]
struct TotalAC {
    #[serde(rename = "Power")]
    power: Option<ValueUnit>,
    #[serde(rename = "YieldDay")]
    yield_day: Option<ValueUnit>,
    #[serde(rename = "YieldTotal")]
    yield_total: Option<ValueUnit>,
}
#[derive(Debug, Deserialize)]
struct ValueUnit { v: f64, u: String }
// ── Zendure 2400AC JSON structs ───────────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct ZendureResponse {
    properties: ZendureProperties,
    #[serde(rename = "packData", default)]
    pack_data: Vec<ZendurePack>,
}
#[derive(Debug, Deserialize, Default)]
struct ZendureProperties {
    #[serde(rename = "electricLevel", default)]  electric_level:    u8,
    #[serde(rename = "packInputPower", default)] pack_input_power:  i32,
    #[serde(rename = "outputPackPower", default)]output_pack_power: i32,
    #[serde(rename = "outputHomePower", default)]output_home_power: i32,
}
#[derive(Debug, Deserialize)]
struct ZendurePack {
    sn: String,
    #[serde(rename = "socLevel", default)] soc_level: u8,
    #[serde(rename = "power",    default)] power:     i32,
}
// ── WiFi ──────────────────────────────────────────────────────────────────────
fn wifi_connect(
    modem: esp_idf_hal::modem::Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
    let esp_wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))?;
    let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().expect("SSID too long"),
        password: WIFI_PASS.try_into().expect("Password too long"),
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    log::info!("WiFi started, connecting to '{}'…", WIFI_SSID);
    wifi.connect()?;
    wifi.wait_netif_up()?;
    log::info!("WiFi connected!");
    Ok(wifi)
}
// ── HTTP helpers ──────────────────────────────────────────────────────────────
fn http_get(url: &str) -> anyhow::Result<Vec<u8>> {
    let mut client = HttpClient::wrap(EspHttpConnection::new(&HttpConfig::default())?);
    let request = client.request(Method::Get, url, &[])?;
    let mut response = request.submit()?;
    let mut body: Vec<u8> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&buf[..n]),
            Err(e) => return Err(anyhow::anyhow!("Read error: {:?}", e)),
        }
    }
    Ok(body)
}
fn fetch_livedata() -> anyhow::Result<LiveData>       { Ok(serde_json::from_slice(&http_get(OPENDTU_URL)?)?) }
fn fetch_zendure()  -> anyhow::Result<ZendureResponse>{ Ok(serde_json::from_slice(&http_get(ZENDURE_URL)?)?) }
// ── Display helpers ───────────────────────────────────────────────────────────
const W: i32 = 320;
const BLACK:  Rgb565 = Rgb565::BLACK;
const WHITE:  Rgb565 = Rgb565::WHITE;
const BLUE:   Rgb565 = Rgb565::new(0, 14, 31);
const GREEN:      Rgb565 = Rgb565::new(0, 31, 0);
const LIGHT_GREEN: Rgb565 = Rgb565::new(15, 31, 15);
const RED:    Rgb565 = Rgb565::new(31, 0, 0);
const YELLOW: Rgb565 = Rgb565::new(31, 31, 0);
const ORANGE: Rgb565 = Rgb565::new(31, 20, 0);
fn fill_rect<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, w: i32, h: i32, color: Rgb565) {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyleBuilder::new().fill_color(color).build())
        .draw(d).ok();
}
fn draw_text<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str, x: i32, y: i32, color: Rgb565, _bold: bool) {
    FontRenderer::new::<fonts::u8g2_font_helvR24_tf>()
        .render_aligned(text, Point::new(x, y), VerticalPosition::Baseline,
            HorizontalAlignment::Left, FontColor::Transparent(color), d).ok();
}
fn draw_text_xl<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str, x: i32, y: i32, color: Rgb565) {
    FontRenderer::new::<fonts::u8g2_font_helvR24_tf>()
        .render_aligned(text, Point::new(x, y), VerticalPosition::Baseline,
            HorizontalAlignment::Center, FontColor::Transparent(color), d).ok();
}
fn draw_text_sm<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str, x: i32, y: i32, color: Rgb565) {
    FontRenderer::new::<fonts::u8g2_font_helvR14_tf>()
        .render_aligned(text, Point::new(x, y), VerticalPosition::Baseline,
            HorizontalAlignment::Left, FontColor::Transparent(color), d).ok();
}
// ── UTF-8 → ASCII ─────────────────────────────────────────────────────────────
fn to_ascii(s: &str) -> String {
    s.chars().map(|c| match c {
        'Ä'|'ä'=>'A','Ö'|'ö'=>'O','Ü'|'ü'=>'U','ß'=>'s',
        'é'|'è'|'ê'=>'e','á'|'à'|'â'=>'a','ó'|'ò'|'ô'=>'o',
        'ú'|'ù'|'û'=>'u','í'|'ì'|'î'=>'i','ñ'=>'n','ç'=>'c',
        c if c.is_ascii() => c, _ => '?',
    }).collect()
}
// ── Screen layout ─────────────────────────────────────────────────────────────
//  0 ┌─────────────────────────────────┐
//    │ Title bar                       │  32px  ← PROFONT_24_POINT (18×30px)
// 32 ├─────────────────────────────────┤
//    │  Total Solar Power  (XL)        │  64px
// 96 ├─────────────────────────────────┤
//    │  [battery bar]                  │   8px
//    │  56%             CHG  309 W     │  46px  ← PROFONT_24_POINT
//142 ├─────────────────────────────────┤
//    │  Per-inverter details           │  66px  (~2 rows × 34px)
//208 ├─────────────────────────────────┤
//    │  Footer: Day / Total yield      │  32px
//240 └─────────────────────────────────┘
fn draw_screen<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    data: &LiveData,
    zendure: Option<&ZendureResponse>,
) {
    fill_rect(display, 0, 0, W, 240, BLACK);
    // ── Title bar ─────────────────────────────────────────────────────────────
    fill_rect(display, 0, 0, W, 32, BLUE);
    draw_text_sm(display, "OpenDTU Monitor", 6, 24, WHITE);
    // ── Big total solar power ─────────────────────────────────────────────────
    fill_rect(display, 0, 32, W, 64, Rgb565::new(0, 6, 12));
    if let Some(p) = &data.total.power {
        draw_text_sm(display, "Total Power", W / 2 - 36, 58, Rgb565::new(16, 24, 31));
        draw_text_xl(display, &format!("{:.0} {}", p.v, p.u), W / 2, 90, LIGHT_GREEN);
    }
    // ── Zendure 2400AC — big font ─────────────────────────────────────────────
    fill_rect(display, 0, 96, W, 46, Rgb565::new(3, 3, 6));
    if let Some(z) = zendure {
        let p = &z.properties;
        let bar_color = match p.electric_level {
            0..=20  => RED,
            21..=50 => ORANGE,
            _       => GREEN,
        };
        // thin battery bar spanning full width
        fill_rect(display, 4, 98, W - 8, 6, Rgb565::new(8, 8, 8));
        let filled = ((W - 8) * p.electric_level as i32) / 100;
        fill_rect(display, 4, 98, filled, 6, bar_color);
        // battery % on left half, CHG/DCH on right half — PROFONT_24_POINT
        let soc_txt = format!("{:.0}%", p.electric_level);
        // packInputPower  = battery feeding power INTO the system = discharging
        // outputPackPower = system sending power INTO the battery = charging
        let (pwr_txt, pwr_color) = if p.output_pack_power > 0 {
            (format!("CHG {} W", p.output_pack_power), GREEN)  // charging    → green
        } else if p.pack_input_power > 0 {
            (format!("DCH {} W", p.pack_input_power), ORANGE)  // discharging → orange
        } else {
            ("IDLE".to_string(), Rgb565::new(16, 16, 16))      // idle        → dim
        };
        draw_text_xl(display, &soc_txt,  W / 4     - 24, 130, bar_color);
        draw_text_xl(display, &pwr_txt,  W / 4 * 3 - 24, 130, pwr_color);
    } else {
        draw_text(display, "Zendure: no data", 4, 130, RED, false);
    }
    // ── Per-inverter rows ─────────────────────────────────────────────────────
    let mut y = 142i32;
    for inv in &data.inverters {
        if y + 34 > 208 { break; }
        let status_color = if inv.producing { GREEN } else if inv.reachable { YELLOW } else { RED };
        let status_txt   = if inv.producing { "ON" } else if inv.reachable { "RCH" } else { "OFF" };
        fill_rect(display, 0, y, W, 20, Rgb565::new(4, 4, 8));
        draw_text_sm(display, &to_ascii(&inv.name), 4, y + 15, WHITE);
        draw_text_sm(display, status_txt, 278, y + 15, status_color);
        y += 20;
        if let Some(ac) = &inv.ac {
            if let Some(ph) = &ac.phase0 {
                if let Some(pw) = &ph.power {
                    if y + 20 <= 208 {
                        fill_rect(display, 0, y, W, 20, Rgb565::new(2, 2, 4));
                        draw_text_sm(display, &format!("  P:{:.1} {}", pw.v, pw.u), 4, y + 15, WHITE);
                        y += 20;
                    }
                }
            }
        }
    }
    // ── Footer ────────────────────────────────────────────────────────────────
    let footer_y = 208i32;
    fill_rect(display, 0, footer_y, W, 32, BLUE);
    if let Some(yd) = &data.total.yield_day   { draw_text_sm(display, &format!("Day: {:.0} {}", yd.v, yd.u),  6,   footer_y + 22, WHITE); }
    if let Some(yt) = &data.total.yield_total { draw_text_sm(display, &format!("Tot: {:.3} {}", yt.v, yt.u),  170, footer_y + 22, WHITE); }
}
// ── Entry point ───────────────────────────────────────────────────────────────
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = Peripherals::take().unwrap();
    let sysloop      = EspSystemEventLoop::take().unwrap();
    let nvs          = EspDefaultNvsPartition::take().unwrap();
    let mut backlight = PinDriver::output(peripherals.pins.gpio21).unwrap();
    backlight.set_high().unwrap();
    let spi_driver = SpiDriver::new(
        peripherals.spi2,
        peripherals.pins.gpio14,
        peripherals.pins.gpio13,
        Some(peripherals.pins.gpio12),
        &SpiDriverConfig::new(),
    ).unwrap();
    let spi_device = SpiDeviceDriver::new(
        spi_driver,
        Some(peripherals.pins.gpio15),
        &SpiConfig::new().baudrate(40_u32.MHz().into()),
    ).unwrap();
    let dc  = PinDriver::output(peripherals.pins.gpio2).unwrap();
    let di  = SPIInterface::new(spi_device, dc);
    let mut delay = Ets;
    let mut display = Builder::new(ILI9341Rgb565, di)
        .orientation(Orientation::new().rotate(Rotation::Deg90).flip_horizontal())
        .color_order(ColorOrder::Bgr)
        .init(&mut delay)
        .unwrap();
    log::info!("Display initialised");
    let _wifi = match wifi_connect(peripherals.modem, sysloop, nvs) {
        Ok(w)  => w,
        Err(e) => { log::error!("WiFi failed: {:?}", e); loop { thread::sleep(Duration::from_secs(5)); } }
    };
    loop {
        let opendtu = fetch_livedata();
        let zendure = fetch_zendure();
        if let Ok(ref data) = opendtu {
            draw_screen(&mut display, data, zendure.as_ref().ok());
            log::info!("Display updated (zendure ok: {})", zendure.is_ok());
        } else {
            log::error!("OpenDTU fetch error: {:?}", opendtu.err());
            fill_rect(&mut display, 0, 90, 320, 20, BLACK);
            draw_text(&mut display, "OpenDTU error - retrying...", 10, 104, RED, false);
        }
        thread::sleep(Duration::from_secs(10));
    }
}
