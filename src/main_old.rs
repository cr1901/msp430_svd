extern crate xmltree;
extern crate svd_parser as svd;

#[macro_use]
mod write;

use std::io::Write;
use xmltree::Element;
use svd::*;

macro_rules! try {
    ($e:expr) => {
        $e.expect(concat!(file!(), ":", line!(), " ", stringify!($e)))
    }
}

fn u32_str(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.starts_with("0x") || text.starts_with("0X") {
        u32::from_str_radix(&text["0x".len()..], 16).ok()
    } else if text.starts_with('#') {
        // Handle strings in the binary form of:
        // #01101x1
        // along with don't care character x (replaced with 0)
        u32::from_str_radix(&str::replace(&text["#".len()..], "x", "0"), 2).ok()
    } else {
        text.parse().ok()
    }
}

trait StringEx {
    fn fix_id(&self) -> String;
}

impl StringEx for String {
    fn fix_id(&self) -> String {
        self.replace("/", "_")
            .replace(" ", "_")
            .replace("__", "_")
            .replace("__", "_")
    }
}

trait ElementExt {
    fn get_attribute<K>(&self, k: K) -> Option<String>
        where String: PartialEq<K> + std::convert::From<K>;
    fn debug(&self);
}

impl ElementExt for Element {
    fn get_attribute<K>(&self, k: K) -> Option<String>
        where String: PartialEq<K> + std::convert::From<K>
    {
        //self.get_child(k).map(|c| try!(c.text.clone()))
        self.attributes.get(&String::from(k)).map(|v| v.clone())
    }

    fn debug(&self) {
        println!("<{} {:?}>", self.name, self.attributes);
        for c in &self.children {
            println!("{}: {:?}", c.name, c.attributes)
        }
        println!("</{}>", self.name);
    }
}

/// Parses the contents of a dslib module file (XML)
pub fn parse_module(xml: &str) -> Option<Peripheral> {
    let tree = &try!(Element::parse(xml.as_bytes()));
    parse_peripheral(tree)
}

/// Parses the contents of a dslib device file (XML)
pub fn parse_device_file(xml: &str) -> Device {
    let tree = &try!(Element::parse(xml.as_bytes()));
    parse_device(tree)
}

fn main() {
    let xml = include_str!("../targetdb/devices/CC430F5123.xml");
    let original = parse_device_file(xml);
    let out_svd = write::write_device(&original);

    let parsed = svd::parse(&out_svd);

    println!("{}", out_svd);

    assert_eq!(original, parsed);
}

fn parse_device(tree: &Element) -> Device {
    assert_eq!(tree.name, "device");

    Device {
        name: try!(tree.get_attribute("partnum")),
        peripherals: try!(tree.get_child("cpu"))
            .children
            .iter()
            .filter_map(parse_instance)
            .collect(),
        defaults: Defaults {
            size: Some(16),
            reset_value: Some(0x0),
            reset_mask: Some(0xffff),
            access: Some(Access::ReadWrite),
        },
    }
}

fn parse_instance(tree: &Element) -> Option<Peripheral> {
    assert_eq!(tree.name, "instance");

    let xmlpath = try!(tree.get_attribute("xmlpath"));
    let xml = try!(tree.get_attribute("xml"));

    use std::io::Read;
    use std::fs::File;
    use std::path::PathBuf;

    let file_path = PathBuf::from("targetdb/devices").join(xmlpath).join(xml);

    let mut file = File::open(file_path).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();

    parse_module(&contents)
}

fn get_addr_info(r: &Register) -> (u32, u32) {
    match r {
        &Register::Single(ref ri) => (ri.address_offset, try!(ri.size)),
        _ => panic!("arrays are not supported"),
    }
}

fn format_register(r: &Register) -> String {
    let mut s = format!("{:?}", r);
    match s.find("access") {
        Some(x) => {
            s.truncate(x);
        }
        None => {}
    }
    s
}

fn parse_peripheral(tree: &Element) -> Option<Peripheral> {
    assert_eq!(tree.name, "module");

    match tree.get_attribute("hidden") {
        Some(ref s) if s == "true" => {
            return None;
        }
        _ => {}
    }

    let base_address = tree.children
        .iter()
        .map(|x| try!(u32_str(&try!(x.get_attribute("offset")))))
        .min()
        .unwrap();

    let mut regs = tree.children
        .iter()
        .filter_map(|x| parse_register(x, base_address))
        .collect::<Vec<_>>();

    regs.sort_by_key(|r| {
                         let (a, _) = get_addr_info(r);
                         a
                     });

    regs = regs.iter()
        .filter_map(|r| {
            let (r_addr, r_size) = get_addr_info(r);

            for x in &regs {
                if x == r {
                    continue;
                }
                let (x_addr, x_size) = get_addr_info(x);
                if x_addr == r_addr && x_size < r_size {
                    writeln!(std::io::stderr(), "Removing    {}", format_register(r)).ok();
                    writeln!(std::io::stderr(), "In favor of {}", format_register(x)).ok();
                    return None;
                }
            }
            Some(r.clone())
        })
        .collect::<Vec<_>>();

    Some(Peripheral {
             name: try!(tree.get_attribute("id")).fix_id(),
             group_name: tree.get_attribute("id"),
             description: tree.get_attribute("description"),
             base_address: base_address,
             interrupt: vec![],
             registers: Some(regs),
             derived_from: None,
         })
}

fn parse_register(tree: &Element, base_address: u32) -> Option<Register> {
    let r = parse_register_info(tree, base_address);
    Some(Register::Single(r))
}

fn parse_register_info(tree: &Element, base_address: u32) -> RegisterInfo {
    assert_eq!(tree.name, "register");

    RegisterInfo {
        name: try!(tree.get_attribute("acronym")).fix_id(),
        description: try!(tree.get_attribute("description")),
        address_offset: try!(u32_str(&try!(tree.get_attribute("offset")))) - base_address,
        size: u32_str(&try!(tree.get_attribute("width"))),
        access: None,
        reset_value: None,
        reset_mask: None,
        fields: match tree.children.len() {
            0 => None,
            _ => {
                Some(tree.children
                         .iter()
                         .map(parse_field)
                         .collect::<Vec<_>>())
            }
        },
        write_constraint: None,
    }
}

fn parse_field(tree: &Element) -> Field {
    assert_eq!(tree.name, "bitfield");

    Field {
        name: try!(tree.get_attribute("id")).fix_id(),
        description: tree.get_attribute("description"),
        bit_range: BitRange {
            offset: try!(u32_str(&try!(tree.get_attribute("begin")))),
            width: try!(u32_str(&try!(tree.get_attribute("width")))),
        },
        access: tree.get_attribute("rwaccess").map(parse_access),
        enumerated_values: match tree.children.len() {
            0 => vec![],
            _ => {
                vec![EnumeratedValues {
                         name: None,
                         usage: None,
                         derived_from: None,
                         values: tree.children
                             .iter()
                             .map(parse_enum)
                             .collect::<Vec<_>>(),
                     }]
            }
        },
        write_constraint: None,
    }
}

fn parse_access(text: String) -> Access {
    match text.as_str() {
        "RW" => Access::ReadWrite,
        "RO" => Access::ReadOnly,
        _ => panic!("bad access"),
    }
}

fn parse_enum(tree: &Element) -> EnumeratedValue {
    assert_eq!(tree.name, "bitenum");

    EnumeratedValue {
        name: try!(tree.get_attribute("id")).fix_id(),
        description: tree.get_attribute("description"),
        value: u32_str(&try!(tree.get_attribute("value"))),
        is_default: None,
    }
}