//! Port of tmux/machine-kind.ts: the machine's kind from the facts line the probe prints. The
//! script that gathers the facts (MACHINE_KIND_SH) stays with the probe in TS.

use inyon_protocol::MachineKind;

use crate::js::to_number;
use crate::re;

/// SMBIOS chassis types of portable machines: portable, laptop, notebook, sub-notebook.
const PORTABLE: [&str; 4] = ["8", "9", "10", "14"];

/// Hypervisors and clouds by the model name they give the machine.
fn virtual_model(model: &str) -> bool {
    re!(r"virtual|vmware|kvm|qemu|hvm|xen|bochs|parallels|virtualbox|google compute|amazon ec2|droplet|openstack", "i").is_match(model)
}

/// The kind from the facts line, or None when the machine gave none (an older probe).
pub fn machine_kind(line: &str) -> Option<MachineKind> {
    use crate::js::JsStr;
    let parts: Vec<&str> = line.jtrim().split('|').collect();
    let f = |i: usize| parts.get(i).copied().unwrap_or("");
    let (os, chassis, host_chassis, virt, graphical, batteries, model, product) = (f(0), f(1), f(2), f(3), f(4), f(5), f(6), f(7));
    if os.is_empty() {
        return None;
    }
    let portable = chassis.split(',').any(|c| PORTABLE.contains(&c.jtrim()));
    let battery = to_number(batteries) > 0.0;
    if os == "Windows" {
        // 2 and 3 are Windows Server (domain controller, server)
        if product == "2" || product == "3" || virtual_model(model) {
            return Some(MachineKind::Server);
        }
        return Some(if portable || battery { MachineKind::Laptop } else { MachineKind::Desktop });
    }
    if os == "Darwin" {
        if virtual_model(model) {
            return Some(MachineKind::Server);
        }
        return Some(if battery || re!(r"macbook", "i").is_match(model) { MachineKind::Laptop } else { MachineKind::Desktop });
    }
    // WSL runs on someone's Windows PC, not in a data centre
    if virt == "wsl" {
        return Some(if battery { MachineKind::Laptop } else { MachineKind::Desktop });
    }
    if (!virt.is_empty() && virt != "none") || re!(r"^(vm|container|server)$").is_match(host_chassis) || virtual_model(model) {
        return Some(MachineKind::Server);
    }
    if graphical != "1" {
        return Some(MachineKind::Server);
    }
    if portable || re!(r"^(laptop|convertible|tablet)$").is_match(host_chassis) || battery {
        return Some(MachineKind::Laptop);
    }
    Some(MachineKind::Desktop)
}

/// describe('machine kind'), from machine-kind.test.ts.
#[cfg(test)]
mod tests {
    use super::*;
    use MachineKind::*;

    /// a cloud VM is a server (this kind of box: KVM, no display)
    #[test]
    fn a_cloud_vm_is_a_server() {
        assert_eq!(machine_kind("Linux|1|vm|kvm|0|0|KVM Virtual Machine|"), Some(Server));
    }

    /// containers and server chassis are servers
    #[test]
    fn containers_and_server_chassis_are_servers() {
        assert_eq!(machine_kind("Linux|||docker|0|0||"), Some(Server));
        assert_eq!(machine_kind("Linux|23|server|none|0|0|PowerEdge R640|"), Some(Server));
    }

    /// Linux with no graphical session is a server, even on desktop hardware
    #[test]
    fn linux_with_no_graphical_session_is_a_server_even_on_desktop_hardware() {
        assert_eq!(machine_kind("Linux|3|desktop|none|0|0|OptiPlex 7090|"), Some(Server));
    }

    /// Linux laptops by chassis type, hostnamectl or battery
    #[test]
    fn linux_laptops_by_chassis_type_hostnamectl_or_battery() {
        for ct in ["8", "9", "10", "14"] {
            assert_eq!(machine_kind(&format!("Linux|{ct}||none|1|0|ThinkPad|")), Some(Laptop));
        }
        assert_eq!(machine_kind("Linux|||none|1|0||"), Some(Desktop));
        assert_eq!(machine_kind("Linux||convertible|none|1|0||"), Some(Laptop));
        assert_eq!(machine_kind("Linux|||none|1|1||"), Some(Laptop));
    }

    /// a Linux desktop with a display is a desktop
    #[test]
    fn a_linux_desktop_with_a_display_is_a_desktop() {
        assert_eq!(machine_kind("Linux|3|desktop|none|1|0|B550 AORUS|"), Some(Desktop));
    }

    /// WSL takes after the PC it runs on
    #[test]
    fn wsl_takes_after_the_pc_it_runs_on() {
        assert_eq!(machine_kind("Linux|||wsl|0|0||"), Some(Desktop));
        assert_eq!(machine_kind("Linux|||wsl|0|1||"), Some(Laptop));
    }

    /// macOS: a battery or a MacBook model is a laptop
    #[test]
    fn macos_a_battery_or_a_macbook_model_is_a_laptop() {
        assert_eq!(machine_kind("Darwin||||1|1|Mac14,2|"), Some(Laptop));
        assert_eq!(machine_kind("Darwin||||1|0|MacBookPro18,3|"), Some(Laptop));
        assert_eq!(machine_kind("Darwin||||1|0|Mac14,3|"), Some(Desktop));
        assert_eq!(machine_kind("Darwin||||1|0|VirtualMac2,1|"), Some(Server));
    }

    /// Windows: server editions and VMs, then chassis or battery
    #[test]
    fn windows_server_editions_and_vms_then_chassis_or_battery() {
        assert_eq!(machine_kind("Windows|3|||1|0|OptiPlex|3"), Some(Server));
        assert_eq!(machine_kind("Windows|1|||1|0|Virtual Machine|1"), Some(Server));
        assert_eq!(machine_kind("Windows|10|||1|1|XPS 13|1"), Some(Laptop));
        assert_eq!(machine_kind("Windows|3,9|||1|0|Surface|1"), Some(Laptop));
        assert_eq!(machine_kind("Windows|3|||1|0|Custom PC|1"), Some(Desktop));
    }

    /// no facts (an older probe) is unknown
    #[test]
    fn no_facts_an_older_probe_is_unknown() {
        assert_eq!(machine_kind(""), None);
    }

    /// The machine-kind block of the probe, as machine-kind.ts has it on its own.
    #[cfg(unix)]
    fn machine_kind_sh() -> &'static str {
        let probe = crate::tmux::tmux::PROBE;
        let from = probe.find("k=\"$HOME/.inyon/machine-kind\"").unwrap();
        let end = "echo \"@@KIND $(head -n 1 \"$k\" 2>/dev/null)\"\n";
        &probe[from..probe.find(end).unwrap() + end.len()]
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_script_runs_here_and_caches_what_it_found() {
        let home = std::env::temp_dir().join(format!("inyon-kind-{}", std::process::id()));
        let run = || {
            let o = std::process::Command::new("sh").arg("-c").arg(machine_kind_sh()).env("HOME", &home).env("DISPLAY", "").env("WAYLAND_DISPLAY", "").output().unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_owned()
        };
        let out = run();
        let cached = std::fs::read_to_string(home.join(".inyon/machine-kind"));
        // the second run reads the file instead of looking again
        std::fs::write(home.join(".inyon/machine-kind"), "Linux|||none|1|1||\n").unwrap();
        let again = run();
        let _ = std::fs::remove_dir_all(&home);
        assert!(regex::Regex::new(r"^@@KIND (Linux|Darwin)\|").unwrap().is_match(&out), "{out}");
        assert!(machine_kind(&out[7..]).is_some());
        assert_eq!(cached.unwrap(), format!("{}\n", &out[7..]));
        assert_eq!(again, "@@KIND Linux|||none|1|1||");
    }
}
