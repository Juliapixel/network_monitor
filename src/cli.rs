use std::{net::{Ipv4Addr, Ipv6Addr}, path::PathBuf, str::FromStr};

use clap::Parser;
use trust_dns_resolver::config::{LookupIpStrategy, ResolverConfig, ResolverOpts};

#[derive(Debug, Clone, Parser)]
#[command(version, author)]
pub struct Args {
    /// interval between ping attempts in seconds
    #[arg(short, long, default_value="15")]
    pub interval: usize,

    /// output directory for logs
    #[arg(short = 'o', long, value_parser=parse_log_file_dir)]
    pub out_dir: Option<PathBuf>,

    /// verbosity
    #[arg(short, action = clap::ArgAction::Count)]
    pub verbosity: u8,

    /// hostname used for pinging
    ///
    /// must be either a valid URL, like "https://youtube.com" or a domain name, like "youtube.com"
    #[arg(default_value="google.com", value_parser=parse_address)]
    pub hostname: (Ipv4Addr, Ipv6Addr)
}

/// errors if not a directory or if path doesn't exist
fn parse_log_file_dir(val: &str) -> Result<PathBuf, &'static str> {
    let path = PathBuf::from(val);
    if !path.exists() {
        return Err("Given path does not exist");
    }
    if !path.is_dir() {
        return Err("Giver path is not a directory");
    }
    match path.canonicalize() {
        Ok(o) => return Ok(o),
        Err(_) => return Err("Could not make path absolute"),
    }
}

/// looks up the A and AAAA records of the given domain, errors if either aren't present
fn parse_address(val: &str) -> Result<(Ipv4Addr, Ipv6Addr), &'static str> {
    let url = url::Url::from_str(val);

    let resolver = trust_dns_resolver::Resolver::new(
        ResolverConfig::cloudflare(),
        {
            let mut opts = ResolverOpts::default();
            opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
            opts
        }
    ).unwrap();

    if let Ok(url) = url {
        let hostname = match url.host().ok_or("The provided URL does not have a domain")? {
            url::Host::Domain(d) => d,
            _ => return Err("The provided URL must have a domain, not an IP address")
        };

        let lookup = resolver.lookup_ip(hostname);

        match lookup {
            Ok(o) => {
                let v4: Option<Ipv4Addr> = o.as_lookup().record_iter().find_map(|r| Some(r.data()?.as_a()?.0));
                let v6: Option<Ipv6Addr> = o.as_lookup().record_iter().find_map(|r| Some(r.data()?.as_aaaa()?.0));

                match (v4, v6) {
                    (None, None) => return Err("The provided domain is invalid"),
                    (None, Some(_)) => return Err("The provided domain does not support IPv4"),
                    (Some(_), None) => return Err("The provided domain does not support IPv6"),
                    (Some(v4), Some(v6)) => return Ok((v4, v6)),
                }
            },
            Err(_e) => return Err("There was an error while trying to resolve the hostname"),
        }
    } else {
        let lookup = resolver.lookup_ip(val);

        match lookup {
            Ok(o) => {
                let v4: Option<Ipv4Addr> = o.as_lookup().record_iter().find_map(|r| Some(r.data()?.as_a()?.0));
                let v6: Option<Ipv6Addr> = o.as_lookup().record_iter().find_map(|r| Some(r.data()?.as_aaaa()?.0));

                match (v4, v6) {
                    (None, None) => return Err("The provided domain is invalid"),
                    (None, Some(_)) => return Err("The provided domain does not support IPv4"),
                    (Some(_), None) => return Err("The provided domain does not support IPv6"),
                    (Some(v4), Some(v6)) => return Ok((v4, v6)),
                }
            },
            Err(_e) => return Err("There was an error while trying to resolve the hostname"),
        }
    }
}
