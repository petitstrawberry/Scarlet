#![cfg_attr(target_os = "scarlet", no_std)]
#![cfg_attr(target_os = "scarlet", no_main)]
#[cfg(target_os = "scarlet")]
extern crate scarlet_std as std;

#[cfg(not(target_os = "scarlet"))]
fn main() {}

#[cfg(target_os = "scarlet")]
mod guest {
    use scarlet_abi::network::{IPV4_HAS_GATEWAY, IPV4_MAKE_DEFAULT};
    use scarlet_os::network::{
        NetworkUpdateIpv4V1, list_interface_configs, list_links, update_link_ipv4,
    };
    use std::{fs::File, io::Write, println, string::String, thread, time::Duration};

    fn wait(label: &str, mut condition: impl FnMut() -> bool) {
        for _ in 0..300 {
            if condition() {
                if label != "INITIAL" {
                    println!("NETWORK_QA {label}");
                }
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("network QA timed out: {label}");
    }
    fn dns() -> String {
        let mut bytes = [0u8; 1024];
        let count = File::open("/etc/resolv.conf")
            .and_then(|mut f| f.read(&mut bytes))
            .unwrap_or(0);
        String::from_utf8_lossy(&bytes[..count]).into_owned()
    }
    fn configured(name: &str) -> bool {
        list_interface_configs()
            .unwrap()
            .iter()
            .any(|record| record.interface_name() == Some(name) && record.ip_set != 0)
    }
    fn preferred(name: &str) -> bool {
        list_interface_configs()
            .unwrap()
            .iter()
            .any(|record| record.interface_name() == Some(name) && record.is_default != 0)
    }
    pub fn run() -> i32 {
        // A short output buffer must not look like a complete one-NIC snapshot.
        let sentinel = scarlet_abi::network::NetworkLinkInfoV1 {
            id: 999,
            ..Default::default()
        };
        let mut short = sentinel;
        let count = unsafe {
            std::syscall::syscall2(
                std::syscall::Syscall::NetworkListLinksV1,
                &mut short as *mut _ as usize,
                1,
            )
        };
        assert_eq!(count, 2);
        assert_eq!(short, sentinel);
        let pid = std::task::fork();
        assert!(pid >= 0);
        if pid == 0 {
            let result = std::task::execve("/bin/netcfgd", &["netcfgd"], &[]);
            panic!("netcfgd exec failed {result}");
        }
        wait("INITIAL", || {
            configured("veth0")
                && configured("veth1")
                && preferred("veth0")
                && dns().matches("nameserver").count() == 2
        });
        let original = list_links()
            .unwrap()
            .into_iter()
            .find(|link| link.interface_name() == Some("veth0"))
            .unwrap();
        let stale = NetworkUpdateIpv4V1 {
            id: original.id,
            generation: original.generation,
            address: [10, 0, 2, 99],
            netmask: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            metric: 10,
            flags: IPV4_HAS_GATEWAY | IPV4_MAKE_DEFAULT,
            reserved: 0,
        };
        assert!(
            update_link_ipv4(&NetworkUpdateIpv4V1 {
                flags: 0x80,
                ..stale
            })
            .is_err()
        );
        assert!(
            update_link_ipv4(&NetworkUpdateIpv4V1 {
                reserved: 1,
                ..stale
            })
            .is_err()
        );
        println!("NETWORK_QA INITIAL");
        wait("DOWN", || {
            !configured("veth0")
                && configured("veth1")
                && preferred("veth1")
                && dns().matches("nameserver").count() == 1
        });
        assert!(
            update_link_ipv4(&stale).is_err(),
            "old lease accepted after carrier loss"
        );
        wait("RESTORED", || {
            configured("veth0") && preferred("veth0") && dns().matches("nameserver").count() == 2
        });
        assert!(
            update_link_ipv4(&stale).is_err(),
            "old lease accepted after reconnect"
        );
        let restored = list_links()
            .unwrap()
            .into_iter()
            .find(|link| link.id == original.id)
            .unwrap();
        assert!(restored.generation > original.generation);
        wait("ALL_DOWN", || {
            let records = list_interface_configs().unwrap();
            records.iter().all(|record| {
                record.ip_set == 0 && record.gateway_set == 0 && record.is_default == 0
            }) && !dns().contains("nameserver")
        });
        wait("FALLBACK_RESTORED", || {
            configured("veth1")
                && !configured("veth0")
                && preferred("veth1")
                && dns().matches("nameserver").count() == 1
        });
        let before_policy = list_links()
            .unwrap()
            .into_iter()
            .find(|link| link.interface_name() == Some("veth1"))
            .unwrap();
        let policy_path = "/etc/netcfgd.d/zz-settings-veth1.toml";
        File::create(policy_path).unwrap().write_all(b"[[interface]]\nname = \"veth1\"\nmethod = \"static\"\naddress = \"10.0.3.77/24\"\ngateway = \"10.0.3.2\"\ndns = [\"9.9.9.9\"]\nmetric = 25\n").unwrap();
        wait("POLICY_STATIC", || {
            list_interface_configs().unwrap().iter().any(|c| {
                c.interface_name() == Some("veth1")
                    && c.ip_set != 0
                    && c.ip_address == [10, 0, 3, 77]
                    && c.metric == 25
            }) && dns().contains("nameserver 9.9.9.9")
                && dns().matches("nameserver").count() == 1
        });
        File::create(policy_path)
            .unwrap()
            .write_all(
                b"[[interface]]\nname = \"veth1\"\nmethod = \"static\"\naddress = \"invalid\"\n",
            )
            .unwrap();
        thread::sleep(Duration::from_secs(3));
        assert!(
            list_interface_configs()
                .unwrap()
                .iter()
                .any(|c| c.interface_name() == Some("veth1") && c.ip_address == [10, 0, 3, 77])
        );
        assert!(dns().contains("nameserver 9.9.9.9"));
        File::create(policy_path)
            .unwrap()
            .write_all(b"[[interface]]\nname = \"veth1\"\nmethod = \"disabled\"\n")
            .unwrap();
        wait("POLICY_DISABLED", || {
            !configured("veth1") && !dns().contains("nameserver")
        });
        File::create(policy_path)
            .unwrap()
            .write_all(b"[[interface]]\nname = \"veth1\"\nmethod = \"dhcp\"\nmetric = 100\n")
            .unwrap();
        wait("POLICY_DHCP", || {
            configured("veth1")
                && dns().matches("nameserver").count() == 1
                && !dns().contains("9.9.9.9")
        });
        let after_policy = list_links()
            .unwrap()
            .into_iter()
            .find(|link| link.id == before_policy.id)
            .unwrap();
        assert_eq!(
            before_policy.generation, after_policy.generation,
            "policy reload must work without fabricating a carrier change"
        );
        println!("NETWORK_QA PASS");
        std::task::shutdown(std::task::ShutdownType::PowerOff);
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    guest::run()
}
