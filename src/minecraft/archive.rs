// https://zh.minecraft.wiki/w/%E7%89%88%E6%9C%AC%E4%BF%A1%E6%81%AF%E6%96%87%E4%BB%B6%E6%A0%BC%E5%BC%8F

use cafebabe::constant_pool::{ConstantPoolItem, LiteralConstant};
use cafebabe::parse_class;
use serde_json::Value;
use std::io::{Read, Seek};
use zip::ZipArchive;

// ref: HMCL org.jackhuang.hmcl.game.GameVersion

fn from_json<R: Read + Seek>(jar: &mut ZipArchive<R>) -> Option<String> {
    let file = jar.by_name("version.json").ok()?;
    let json_value: Value = serde_json::from_reader(file).ok()?;
    json_value
        .get("id")
        .and_then(Value::as_str)
        .map(|id| id.split(" / ").next().unwrap_or(id).to_string())
}

fn from_client<R: Read + Seek>(jar: &mut ZipArchive<R>) -> Option<String> {
    let mut file = jar.by_name("net/minecraft/client/Minecraft.class").ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let class = parse_class(&buffer).ok()?;
    class
        .constantpool_iter()
        .filter_map(|item| match item {
            ConstantPoolItem::LiteralConstant(LiteralConstant::String(s)) => Some(s),
            _ => None,
        })
        .find_map(|s| {
            s.strip_prefix("Minecraft Minecraft ")
                .map(|ver| ver.to_string())
        })
}

fn from_server<R: Read + Seek>(jar: &mut ZipArchive<R>) -> Option<String> {
    let mut file = jar
        .by_name("net/minecraft/server/MinecraftServer.class")
        .ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let class = parse_class(&buffer).ok()?;
    let strings: Vec<_> = class
        .constantpool_iter()
        .filter_map(|item| match item {
            ConstantPoolItem::LiteralConstant(LiteralConstant::String(s)) => Some(s),
            _ => None,
        })
        .collect();

    let idx = strings
        .iter()
        .position(|s| s.starts_with("Can't keep up!"))?;

    strings[..idx]
        .iter()
        .rev()
        .find(|s| s.chars().any(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
}

pub fn load_game_version_from_jar<R: Read + Seek>(reader: R) -> Option<String> {
    let mut jar = ZipArchive::new(reader).ok()?;
    if let Some(version) = from_json(&mut jar) {
        return Some(version);
    }
    if let Some(version) = from_client(&mut jar) {
        if let Some(beta) = version.strip_prefix("Beta ") {
            return Some(format!("b{beta}"));
        }
        return Some(version);
    }
    if let Some(version) = from_server(&mut jar) {
        return Some(version);
    }
    None
}
