//! DNS response validation and bounded, TTL-aware positive caching.
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolverConfig {
    pub nameservers: Vec<Ipv4Addr>,
    pub search_domains: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Answer {
    pub addresses: Vec<Ipv4Addr>,
    pub ttl: Duration,
}

#[derive(Default)]
pub(super) struct Cache {
    config: Option<ResolverConfig>,
    entries: HashMap<String, (Instant, Vec<Ipv4Addr>)>,
}

impl Cache {
    fn configure(&mut self, config: &ResolverConfig) {
        if self.config.as_ref() != Some(config) {
            self.entries.clear();
            self.config = Some(config.clone());
        }
    }
    pub fn get(
        &mut self,
        config: &ResolverConfig,
        host: &str,
        now: Instant,
    ) -> Option<Vec<Ipv4Addr>> {
        self.configure(config);
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self.entries.get(&host) {
            Some((expires, addresses)) if *expires > now => Some(addresses.clone()),
            _ => {
                self.entries.remove(&host);
                None
            }
        }
    }
    pub fn insert(&mut self, config: &ResolverConfig, host: &str, answer: &Answer, now: Instant) {
        self.configure(config);
        if answer.ttl.is_zero() || answer.addresses.is_empty() {
            return;
        }
        self.entries.retain(|_, (expires, _)| *expires > now);
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.entries.len() >= 256 && !self.entries.contains_key(&host) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (expires, _))| *expires)
                .map(|(host, _)| host.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        // Expiring early is safe; a large upstream TTL cannot retain entries
        // indefinitely. No negative answers or transport failures are cached.
        let Some(expires) = now.checked_add(answer.ttl.min(Duration::from_secs(3600))) else {
            return;
        };
        self.entries
            .insert(host, (expires, answer.addresses.clone()));
    }
}

fn u16_at(packet: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = packet
        .get(offset..offset + 2)
        .ok_or("truncated DNS integer")?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn name_at(packet: &[u8], mut cursor: usize) -> Result<(String, usize), String> {
    let mut end = None;
    let mut name = String::new();
    for _ in 0..128 {
        let &length = packet.get(cursor).ok_or("truncated DNS name")?;
        if length & 0xc0 == 0xc0 {
            let target = (u16_at(packet, cursor)? & 0x3fff) as usize;
            // Compression refers to a previous occurrence; this also rejects
            // self/forward pointers without following an untrusted cycle.
            if target >= cursor {
                return Err("invalid DNS compression pointer".into());
            }
            end.get_or_insert(cursor + 2);
            cursor = target;
        } else if length == 0 {
            return Ok((name, end.unwrap_or(cursor + 1)));
        } else {
            if length & 0xc0 != 0 {
                return Err("unsupported DNS label".into());
            }
            let label = packet
                .get(cursor + 1..cursor + 1 + length as usize)
                .ok_or("truncated DNS label")?;
            if !label.is_ascii() || label.contains(&0) || label.contains(&b'.') {
                return Err("invalid DNS label".into());
            }
            if !name.is_empty() {
                name.push('.');
            }
            for byte in label {
                name.push(byte.to_ascii_lowercase() as char);
            }
            if name.len() > 253 {
                return Err("DNS name too long".into());
            }
            cursor += 1 + length as usize;
        }
    }
    Err("DNS compression chain too long".into())
}

enum Data {
    A(Ipv4Addr),
    Cname(String),
}
struct Record {
    owner: String,
    ttl: u32,
    data: Data,
}

pub(super) fn parse_a_response(packet: &[u8], host: &str, id: u16) -> Result<Answer, String> {
    if packet.len() < 12 {
        return Err("short DNS response".into());
    }
    if u16_at(packet, 0)? != id {
        return Err("DNS response id mismatch".into());
    }
    let flags = u16_at(packet, 2)?;
    if flags & 0xf800 != 0x8000 {
        return Err("not a standard DNS response".into());
    }
    if flags & 0x0200 != 0 {
        return Err("truncated DNS response".into());
    }
    if flags & 0x000f != 0 {
        return Err(format!("DNS error rcode={}", flags & 0xf));
    }
    if u16_at(packet, 4)? != 1 {
        return Err("DNS question count mismatch".into());
    }
    let expected = host.trim_end_matches('.').to_ascii_lowercase();
    let (question, mut offset) = name_at(packet, 12)?;
    if question != expected || u16_at(packet, offset)? != 1 || u16_at(packet, offset + 2)? != 1 {
        return Err("DNS question mismatch".into());
    }
    offset += 4;
    let mut records = Vec::new();
    for _ in 0..u16_at(packet, 6)? {
        let (owner, next) = name_at(packet, offset)?;
        offset = next;
        let record_type = u16_at(packet, offset)?;
        let class = u16_at(packet, offset + 2)?;
        let ttl_bytes = packet
            .get(offset + 4..offset + 8)
            .ok_or("truncated DNS TTL")?;
        let ttl = u32::from_be_bytes(ttl_bytes.try_into().unwrap());
        let ttl = if ttl > i32::MAX as u32 { 0 } else { ttl };
        let length = u16_at(packet, offset + 8)? as usize;
        offset += 10;
        let bytes = packet
            .get(offset..offset + length)
            .ok_or("truncated DNS rdata")?;
        if class == 1 {
            let data = match record_type {
                1 if length == 4 => Some(Data::A(Ipv4Addr::new(
                    bytes[0], bytes[1], bytes[2], bytes[3],
                ))),
                5 => {
                    let (target, end) = name_at(packet, offset)?;
                    if end != offset + length {
                        return Err("invalid CNAME length".into());
                    }
                    Some(Data::Cname(target))
                }
                _ => None,
            };
            if let Some(data) = data {
                records.push(Record { owner, ttl, data });
            }
        }
        offset += length;
    }
    let mut owner = expected;
    let mut ttl = u32::MAX;
    let mut visited = HashSet::new();
    for _ in 0..16 {
        if !visited.insert(owner.clone()) {
            return Err("DNS CNAME cycle".into());
        }
        let aliases: Vec<_> = records
            .iter()
            .filter(|r| r.owner == owner && matches!(r.data, Data::Cname(_)))
            .collect();
        if let Some(alias) = aliases.first() {
            let Data::Cname(target) = &alias.data else {
                unreachable!()
            };
            for alias in &aliases {
                if !matches!(&alias.data, Data::Cname(other) if other == target) {
                    return Err("conflicting CNAME records".into());
                }
                ttl = ttl.min(alias.ttl);
            }
            owner = target.clone();
            continue;
        }
        let mut addresses = Vec::new();
        for record in &records {
            if record.owner == owner {
                if let Data::A(address) = record.data {
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                    ttl = ttl.min(record.ttl);
                }
            }
        }
        if addresses.is_empty() {
            ttl = 0;
        }
        return Ok(Answer {
            addresses,
            ttl: Duration::from_secs(ttl as u64),
        });
    }
    Err("DNS CNAME chain too long".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn name(host: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for label in host.split('.') {
            bytes.push(label.len() as u8);
            bytes.extend_from_slice(label.as_bytes());
        }
        bytes.push(0);
        bytes
    }
    fn response(records: &[(&str, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = vec![
            0x53,
            0x43,
            0x81,
            0x80,
            0,
            1,
            0,
            records.len() as u8,
            0,
            0,
            0,
            0,
        ];
        bytes.extend(name("www.example"));
        bytes.extend([0, 1, 0, 1]);
        for (owner, kind, ttl, data) in records {
            bytes.extend(name(owner));
            bytes.extend(kind.to_be_bytes());
            bytes.extend([0, 1]);
            bytes.extend(ttl.to_be_bytes());
            bytes.extend((data.len() as u16).to_be_bytes());
            bytes.extend(data);
        }
        bytes
    }
    #[test]
    fn follows_aliases_and_uses_the_shortest_relevant_ttl() {
        let packet = response(&[
            ("www.example", 5, 4, name("edge.example")),
            ("edge.example", 1, 60, vec![1, 2, 3, 4]),
            ("unrelated.example", 1, 1, vec![9, 9, 9, 9]),
        ]);
        let answer = parse_a_response(&packet, "WWW.EXAMPLE.", 0x5343).unwrap();
        assert_eq!(answer.addresses, [Ipv4Addr::new(1, 2, 3, 4)]);
        assert_eq!(answer.ttl, Duration::from_secs(4));
    }
    #[test]
    fn rejects_wrong_question_id_truncation_and_alias_cycles() {
        let packet = response(&[
            ("www.example", 5, 10, name("other.example")),
            ("other.example", 5, 10, name("www.example")),
        ]);
        assert!(parse_a_response(&packet, "www.example", 0x5343).is_err());
        assert!(parse_a_response(&packet, "wrong.example", 0x5343).is_err());
        assert!(parse_a_response(&packet, "www.example", 0x5344).is_err());
        let mut packet = response(&[]);
        packet[2] |= 2;
        assert!(parse_a_response(&packet, "www.example", 0x5343).is_err());
        assert!(name_at(&[0xc0, 0], 0).is_err());
        assert!(name_at(&[0xc0, 4, 0], 0).is_err());
    }
    #[test]
    fn cache_expires_alias_ttls_and_invalidates_configuration_changes() {
        let mut cache = Cache::default();
        let now = Instant::now();
        let mut config = ResolverConfig {
            nameservers: vec![Ipv4Addr::new(10, 0, 2, 3)],
            search_domains: vec![],
        };
        let answer = Answer {
            addresses: vec![Ipv4Addr::new(1, 2, 3, 4)],
            ttl: Duration::from_secs(4),
        };
        cache.insert(&config, "WWW.EXAMPLE.", &answer, now);
        assert!(
            cache
                .get(&config, "www.example", now + Duration::from_secs(3))
                .is_some()
        );
        assert!(
            cache
                .get(&config, "www.example", now + Duration::from_secs(4))
                .is_none()
        );
        cache.insert(&config, "www.example", &answer, now);
        config.search_domains.push("local".into());
        assert!(cache.get(&config, "www.example", now).is_none());
        cache.insert(&config, "www.example", &answer, now);
        config.nameservers[0] = Ipv4Addr::new(1, 1, 1, 1);
        assert!(cache.get(&config, "www.example", now).is_none());
    }
    #[test]
    fn zero_ttl_and_missing_answers_are_not_cached_and_cache_is_bounded() {
        let mut cache = Cache::default();
        let now = Instant::now();
        let config = ResolverConfig {
            nameservers: vec![],
            search_domains: vec![],
        };
        let mut answer = Answer {
            addresses: vec![Ipv4Addr::LOCALHOST],
            ttl: Duration::ZERO,
        };
        cache.insert(&config, "zero", &answer, now);
        assert!(cache.get(&config, "zero", now).is_none());
        answer.ttl = Duration::from_secs(30);
        let empty = Answer {
            addresses: vec![],
            ttl: answer.ttl,
        };
        cache.insert(&config, "empty", &empty, now);
        assert!(cache.get(&config, "empty", now).is_none());
        for i in 0..300 {
            cache.insert(&config, &format!("host{i}"), &answer, now);
        }
        assert_eq!(cache.entries.len(), 256);
    }
    #[test]
    fn compressed_names_are_validated_and_ttl_zero_propagates() {
        let mut packet = response(&[]);
        packet[7] = 1;
        packet.extend([0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 1, 2, 3, 4]);
        let answer = parse_a_response(&packet, "www.example", 0x5343).unwrap();
        assert_eq!(answer.addresses.len(), 1);
        assert!(answer.ttl.is_zero());
        packet.pop();
        assert!(parse_a_response(&packet, "www.example", 0x5343).is_err());
    }
}
