//! The standard library: parts callable from every script (`R("10k")`, `NE555()`) and the
//! built-in rails (`GND`, `P3V3`, …).

use crate::ir::{PartDef, PartPin};
use crate::symbol::boxgen::Side;
use crate::symbol::{PinType, RailKind};

pub struct StdPart {
    pub name: &'static str,
    pub prefix: &'static str,
    pub symbol: &'static str,
    pub value: &'static str,
    pub footprint: &'static str,
    pub description: &'static str,
    pub datasheet: &'static str,
    /// (numbers, name, type, side)
    pub pins: &'static [(&'static [&'static str], &'static str, PinType, Option<Side>)],
}

use PinType::*;

const fn two(a: &'static str, b: &'static str) -> [(&'static [&'static str], &'static str, PinType, Option<Side>); 2] {
    [(&["1"], a, Passive, None), (&["2"], b, Passive, None)]
}

const PASSIVE2: [(&[&str], &str, PinType, Option<Side>); 2] = two("~", "~");
const NUMBERED2: [(&[&str], &str, PinType, Option<Side>); 2] = two("1", "2");
const DIODE: [(&[&str], &str, PinType, Option<Side>); 2] = two("K", "A");
const POLAR: [(&[&str], &str, PinType, Option<Side>); 2] = two("+", "-");
const BJT: [(&[&str], &str, PinType, Option<Side>); 3] = [(&["1"], "B", Input, None), (&["3"], "C", Passive, None), (&["2"], "E", Passive, None)];
const FET: [(&[&str], &str, PinType, Option<Side>); 3] = [(&["1"], "G", Input, None), (&["3"], "D", Passive, None), (&["2"], "S", Passive, None)];

pub const PARTS: &[StdPart] = &[
    StdPart { name: "R", prefix: "R", symbol: "resistor", value: "10k", footprint: "Resistor_SMD:R_0603_1608Metric", description: "Resistor", datasheet: "", pins: &PASSIVE2 },
    StdPart { name: "C", prefix: "C", symbol: "capacitor", value: "100nF", footprint: "Capacitor_SMD:C_0603_1608Metric", description: "Unpolarized capacitor", datasheet: "", pins: &PASSIVE2 },
    StdPart { name: "C_Pol", prefix: "C", symbol: "cap_polarized", value: "10uF", footprint: "Capacitor_THT:CP_Radial_D5.0mm_P2.00mm", description: "Polarized capacitor (electrolytic, tantalum)", datasheet: "", pins: &POLAR },
    StdPart { name: "L", prefix: "L", symbol: "inductor", value: "10uH", footprint: "Inductor_SMD:L_0805_2012Metric", description: "Inductor", datasheet: "", pins: &PASSIVE2 },
    StdPart { name: "Fuse", prefix: "F", symbol: "fuse", value: "500mA", footprint: "Fuse:Fuse_1206_3216Metric", description: "Fuse or polyfuse", datasheet: "", pins: &PASSIVE2 },
    StdPart { name: "Crystal", prefix: "Y", symbol: "crystal", value: "16MHz", footprint: "Crystal:Crystal_HC49-4H_Vertical", description: "Two-pin crystal", datasheet: "", pins: &NUMBERED2 },
    StdPart { name: "D", prefix: "D", symbol: "diode", value: "1N4148W", footprint: "Diode_SMD:D_SOD-123", description: "Diode", datasheet: "", pins: &DIODE },
    StdPart { name: "D_Schottky", prefix: "D", symbol: "schottky", value: "BAT54", footprint: "Diode_SMD:D_SOD-123", description: "Schottky diode", datasheet: "", pins: &DIODE },
    StdPart { name: "D_Zener", prefix: "D", symbol: "zener", value: "BZX84C5V1", footprint: "Diode_SMD:D_SOD-123", description: "Zener diode", datasheet: "", pins: &DIODE },
    StdPart { name: "LED", prefix: "D", symbol: "led", value: "LED", footprint: "LED_SMD:LED_0603_1608Metric", description: "Light emitting diode", datasheet: "", pins: &DIODE },
    StdPart { name: "Q_NPN", prefix: "Q", symbol: "npn", value: "MMBT3904", footprint: "Package_TO_SOT_SMD:SOT-23", description: "NPN transistor", datasheet: "", pins: &BJT },
    StdPart { name: "Q_PNP", prefix: "Q", symbol: "pnp", value: "MMBT3906", footprint: "Package_TO_SOT_SMD:SOT-23", description: "PNP transistor", datasheet: "", pins: &BJT },
    StdPart { name: "Q_NMOS", prefix: "Q", symbol: "nmos", value: "2N7002", footprint: "Package_TO_SOT_SMD:SOT-23", description: "N-channel MOSFET", datasheet: "", pins: &FET },
    StdPart { name: "Q_PMOS", prefix: "Q", symbol: "pmos", value: "AO3401A", footprint: "Package_TO_SOT_SMD:SOT-23", description: "P-channel MOSFET", datasheet: "", pins: &FET },
    StdPart { name: "SW_Push", prefix: "SW", symbol: "button", value: "SW_Push", footprint: "Button_Switch_SMD:SW_SPST_TL3342", description: "Push button, normally open", datasheet: "", pins: &NUMBERED2 },
    StdPart { name: "SW_SPST", prefix: "SW", symbol: "switch", value: "SW_SPST", footprint: "Button_Switch_THT:SW_Slide_1P2T_CK_OS102011MS2Q", description: "Single pole single throw switch", datasheet: "", pins: &NUMBERED2 },
    StdPart {
        name: "AMS1117_3V3",
        prefix: "U",
        symbol: "box",
        value: "AMS1117-3.3",
        footprint: "Package_TO_SOT_SMD:SOT-223-3_TabPin2",
        description: "1A low dropout regulator, 3.3V",
        datasheet: "http://www.advanced-monolithic.com/pdf/ds1117.pdf",
        pins: &[(&["3"], "VIN", PowerIn, None), (&["2"], "VOUT", PowerOut, None), (&["1"], "GND", PowerIn, None)],
    },
    StdPart {
        name: "NE555",
        prefix: "U",
        symbol: "box",
        value: "NE555",
        footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
        description: "Precision timer",
        datasheet: "",
        pins: &[
            (&["2"], "TR", Input, None),
            (&["6"], "THR", Input, None),
            (&["5"], "CV", Input, None),
            (&["4"], "R", Input, None),
            (&["3"], "Q", Output, None),
            (&["7"], "DIS", OpenCollector, None),
            (&["8"], "VCC", PowerIn, None),
            (&["1"], "GND", PowerIn, None),
        ],
    },
    StdPart {
        name: "USB_C_Power",
        prefix: "J",
        symbol: "box",
        value: "USB_C_Receptacle_PowerOnly_6P",
        footprint: "Connector_USB:USB_C_Receptacle_GCT_USB4125-xx-x-0190_6P_TopMnt_Horizontal",
        description: "USB Type-C receptacle, power only (6 pins)",
        datasheet: "",
        pins: &[
            (&["A9", "B9"], "VBUS", Passive, Some(Side::Right)),
            (&["A5"], "CC1", Bidirectional, Some(Side::Right)),
            (&["B5"], "CC2", Bidirectional, Some(Side::Right)),
            (&["A12", "B12"], "GND", Passive, Some(Side::Right)),
            (&["S1"], "SHIELD", Passive, Some(Side::Right)),
        ],
    },
];

/// Built-in rails: (script constant, net name, kind).
pub const RAILS: &[(&str, &str, RailKind)] = &[
    ("GND", "GND", RailKind::Ground),
    ("AGND", "AGND", RailKind::Ground),
    ("DGND", "DGND", RailKind::Ground),
    ("PGND", "PGND", RailKind::Ground),
    ("P1V8", "+1V8", RailKind::Supply),
    ("P2V5", "+2V5", RailKind::Supply),
    ("P3V3", "+3V3", RailKind::Supply),
    ("P5V", "+5V", RailKind::Supply),
    ("P12V", "+12V", RailKind::Supply),
    ("VBUS", "VBUS", RailKind::Supply),
    ("VBAT", "VBAT", RailKind::Supply),
    ("VCC", "VCC", RailKind::Supply),
    ("VDD", "VDD", RailKind::Supply),
    ("N5V", "-5V", RailKind::Negative),
    ("N12V", "-12V", RailKind::Negative),
];

pub fn find(name: &str) -> Option<&'static StdPart> {
    PARTS.iter().find(|p| p.name == name)
}

impl StdPart {
    pub fn to_def(&self) -> PartDef {
        let mut pins = Vec::new();
        for (numbers, name, ptype, side) in self.pins {
            for n in *numbers {
                pins.push(PartPin { number: n.to_string(), name: name.to_string(), ptype: *ptype, side: *side, hidden: false });
            }
        }
        PartDef {
            name: self.name.into(),
            prefix: self.prefix.into(),
            footprint: self.footprint.into(),
            value: self.value.into(),
            description: self.description.into(),
            datasheet: self.datasheet.into(),
            mpn: String::new(),
            fields: vec![],
            symbol: self.symbol.into(),
            pins,
            open_pins_ok: false,
            span: None,
        }
    }
}
