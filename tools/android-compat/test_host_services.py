from copy import deepcopy
import unittest

from host_services import (BINARY, CONTRACT, MANAGED_RC, STOCK_TEXT, build_suspend,
                           INSTALLER,INSTALLER_BINARY,INSTALLER_MANAGED_RC,build_installer,
                           HW_MANAGER,HW_BINARY,HW_MANAGED_RC,build_hwmanager,
                           imports, validate_selection, validate_staged)
from host_services import HEALTH, HEALTH_BINARY, HEALTH_IMPL, HEALTH_MANAGED_RC, HEALTH_TEXT, build_health
from vintf_inputs import HEALTH_CONTRACT


class HostServicesTest(unittest.TestCase):
    def test_audio_policy_keeps_only_the_primary_stock_graph(self):
        from host_services import primary_audio_policy
        stock=(b'<audioPolicyConfiguration>\n'
               b'<xi:include href="primary_audio_policy_configuration.xml"/>\n'
               b'<xi:include href="audio_policy_volumes.xml"/>\n'
               b'<xi:include href="default_volume_tables.xml"/>\n'
               b'<xi:include href="a2dp_in_audio_policy_configuration_7_0.xml"/>\n'
               b'<xi:include href="r_submix_audio_policy_configuration.xml"/>\n'
               b'<xi:include href="bluetooth_audio_policy_configuration_7_0.xml"/>\n'
               b'</audioPolicyConfiguration>')
        filtered=primary_audio_policy(stock)
        self.assertIn(b'primary_audio_policy_configuration.xml',filtered)
        self.assertIn(b'default_volume_tables.xml',filtered)
        self.assertNotIn(b'a2dp_in_',filtered)
        self.assertNotIn(b'r_submix_',filtered)
        self.assertNotIn(b'bluetooth_audio_',filtered)
        with self.assertRaisesRegex(ValueError,"changed stock audio policy"):
            primary_audio_policy(stock.replace(b'default_volume_tables.xml',b'other.xml'))

    def test_vold_keeps_storage_provider_but_delegates_host_reboot(self):
        from host_services import VOLD,VOLD_BINARY,VOLD_MANAGED_RC,VOLD_TEXT,build_vold
        managed,proof=build_vold(VOLD_TEXT.encode(),b"stock vold")
        self.assertNotIn(b"reboot_on_failure",managed)
        self.assertNotIn(b"ioprio",managed)
        self.assertIn(b"task_profiles ProcessCapacityHigh",managed)
        proof.update(files={VOLD_BINARY:proof["binary_sha256"],"system/lib64/libvold.so":"dependency"},
                     native_needed={VOLD_BINARY:["libvold.so"],"system/lib64/libvold.so":[]})
        metadata={"profile":{"host_services":[VOLD]},"host_service_inputs":{VOLD:proof},
                  "files":{**proof["files"],VOLD_MANAGED_RC:proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        for changed in ["dependency","reboot"]:
            bad=deepcopy(metadata)
            if changed=="dependency":bad["files"]["system/lib64/libvold.so"]="changed"
            else:bad["host_service_inputs"][VOLD]["host_reboot"]=True
            with self.assertRaises(ValueError):validate_staged(bad)
        with self.assertRaises(ValueError):build_vold(VOLD_TEXT.replace("class core","class main").encode(),b"stock")

    def test_lmkd_preserves_stock_socket_and_native_dependencies(self):
        from host_services import LMKD,LMKD_BINARY,LMKD_MANAGED_RC,LMKD_TEXT,build_lmkd
        managed,proof=build_lmkd(LMKD_TEXT.encode(),b"stock lmkd")
        self.assertIn(b"socket lmkd seqpacket+passcred 0660 system system",managed)
        self.assertNotIn(b"capabilities",managed)
        proof.update(files={LMKD_BINARY:proof["binary_sha256"],"system/lib64/libpsi.so":"dependency"},
                     native_needed={LMKD_BINARY:["libpsi.so"],"system/lib64/libpsi.so":[]})
        metadata={"profile":{"host_services":[LMKD]},"host_service_inputs":{LMKD:proof},
                  "files":{**proof["files"],LMKD_MANAGED_RC:proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        bad=deepcopy(metadata);bad["files"]["system/lib64/libpsi.so"]="changed"
        with self.assertRaises(ValueError):validate_staged(bad)
        with self.assertRaises(ValueError):build_lmkd(b"service lmkd /system/bin/lmkd\n",b"stock")

    def test_netd_preserves_control_sockets_and_removes_unsupported_capabilities(self):
        from host_services import NETD,NETD_BINARY,NETD_MANAGED_RC,build_netd
        stock=(b"service netd /system/bin/netd\n    class main\n"
               b"    capabilities CHOWN DAC_OVERRIDE DAC_READ_SEARCH FOWNER IPC_LOCK KILL NET_ADMIN NET_BIND_SERVICE NET_RAW SETUID SETGID\n"
               b"    group root net_admin\n    socket dnsproxyd stream 0660 root inet\n"
               b"    onrestart restart zygote\n    onrestart restart zygote_secondary\n")
        managed,proof=build_netd(stock,b"stock netd")
        self.assertNotIn(b"capabilities",managed)
        self.assertIn(b"socket dnsproxyd",managed)
        self.assertIn(b"setprop bpf.progs_loaded 1",managed)
        self.assertIn(b"oneshot\n    disabled",managed)
        self.assertNotIn(b"onrestart restart zygote",managed)
        self.assertEqual(proof["bpf_loader"],"unavailable")
        proof.update(files={NETD_BINARY:proof["binary_sha256"],"system/lib64/libnetd_resolv.so":"dependency"},
                     native_needed={NETD_BINARY:["libnetd_resolv.so"],"system/lib64/libnetd_resolv.so":[]})
        metadata={"profile":{"host_services":[NETD]},"host_service_inputs":{NETD:proof},
                  "files":{**proof["files"],NETD_MANAGED_RC:proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        bad=deepcopy(metadata);bad["files"]["system/lib64/libnetd_resolv.so"]="changed"
        with self.assertRaises(ValueError):validate_staged(bad)

    def test_health_requires_real_provider_and_preserves_host_power_boundary(self):
        manifest=b'<manifest type="device"><hal format="hidl"><name>android.hardware.health</name><transport>hwbinder</transport><fqname>@2.1::IHealth/default</fqname></hal></manifest>'
        managed,proof,providers=build_health(HEALTH_TEXT.encode(),b"stock health",manifest)
        self.assertNotIn(b"capabilities",managed)
        self.assertIn(b"file /dev/kmsg w",managed)
        profile={"host_services":[HW_MANAGER,HEALTH],"vintf":{"framework":[],"device":[HEALTH_CONTRACT]}}
        self.assertEqual(validate_selection(profile),[HW_MANAGER,HEALTH])
        self.assertEqual(providers[HEALTH_CONTRACT]["path"],HEALTH_BINARY)
        proof["implementation_sha256"]="stock implementation"
        _,manager=build_hwmanager(b"service hwservicemanager /system/bin/hwservicemanager\n",b"manager")
        metadata={"profile":profile,"host_service_inputs":{HEALTH:proof,HW_MANAGER:manager},
                  "files":{HEALTH_BINARY:proof["binary_sha256"],HEALTH_MANAGED_RC:proof["managed_rc_sha256"],
                           HEALTH_IMPL:proof["implementation_sha256"],
                           HW_BINARY:manager["binary_sha256"],HW_MANAGED_RC:manager["managed_rc_sha256"]}}
        validate_staged(metadata)
        for name in [HEALTH_BINARY,HEALTH_MANAGED_RC,HEALTH_IMPL]:
            bad=deepcopy(metadata);bad["files"][name]="changed"
            with self.assertRaises(ValueError):validate_staged(bad)
        bad=deepcopy(metadata);bad["profile"]["host_services"].remove(HW_MANAGER)
        with self.assertRaises(ValueError):validate_selection(bad["profile"])
        with self.assertRaises(ValueError):build_health(HEALTH_TEXT.replace("user system","user root").encode(),b"stock",manifest)
        with self.assertRaises(ValueError):build_health(HEALTH_TEXT.encode(),b"stock",manifest.replace(b"@2.1",b"@2.0"))

    def test_installer_keeps_stock_process_and_rejects_changed_inputs(self):
        rc=b"\nservice installd /system/bin/installd\n    class main\n"
        managed,proof=build_installer(rc,b"stock installd")
        self.assertEqual(managed,rc)
        metadata={"profile":{"host_services":[INSTALLER]},"host_service_inputs":{INSTALLER:proof},
                  "files":{INSTALLER_BINARY:proof["binary_sha256"],INSTALLER_MANAGED_RC:proof["managed_rc_sha256"]}}
        self.assertEqual(imports(metadata["profile"]),["/"+INSTALLER_MANAGED_RC])
        validate_staged(metadata)
        metadata["files"][INSTALLER_BINARY]="changed"
        with self.assertRaises(ValueError):validate_staged(metadata)
        with self.assertRaises(ValueError):build_installer(rc.replace(b"/system/bin/installd",b"/other"),b"binary")
    def test_hwmanager_preserves_the_verified_stock_declaration(self):
        rc=b"service hwservicemanager /system/bin/hwservicemanager\n    user system\n    disabled\n"
        managed,proof=build_hwmanager(rc,b"verified hwmanager")
        self.assertEqual(managed,rc)
        profile={"host_services":[HW_MANAGER]}
        self.assertEqual(imports(profile),["/"+HW_MANAGED_RC])
        metadata={"profile":profile,"host_service_inputs":{HW_MANAGER:proof},
                  "files":{HW_BINARY:proof["binary_sha256"],HW_MANAGED_RC:proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        metadata["files"][HW_MANAGED_RC]="changed"
        with self.assertRaises(ValueError):validate_staged(metadata)
        with self.assertRaises(ValueError):build_hwmanager(b"service wrong /bin/wrong\n",b"binary")
    def test_suspend_requires_provider_and_matching_contract(self):
        profile = {"host_services": ["system_suspend"],
                   "vintf": {"framework": [CONTRACT], "device": []}}
        self.assertEqual(imports(profile), ["/" + MANAGED_RC])
        for key in ("host_services", "vintf"):
            invalid = deepcopy(profile)
            invalid.pop(key)
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_selection(invalid)
        with self.assertRaises(ValueError):
            validate_selection({"host_services": ["unknown"]})

    def test_stock_rc_is_checked_and_unsupported_capability_is_removed(self):
        managed, proof = build_suspend(STOCK_TEXT.encode(), b"verified binary")
        self.assertIn(b"class early_hal", managed)
        self.assertIn(b"user system", managed)
        self.assertNotIn(b"capabilities", managed)
        self.assertFalse(proof["host_suspend"])
        with self.assertRaises(ValueError):
            build_suspend(STOCK_TEXT.replace("user system", "user root").encode(), b"binary")

    def test_staging_checks_provider_and_generated_configuration(self):
        _, proof = build_suspend(STOCK_TEXT.encode(), b"binary")
        metadata = {"profile": {"host_services": ["system_suspend"],
                               "vintf": {"framework": [CONTRACT], "device": []}},
                    "host_service_inputs": {"system_suspend": proof},
                    "files": {BINARY: proof["binary_sha256"], MANAGED_RC: proof["managed_rc_sha256"]}}
        validate_staged(metadata)
        for field in (BINARY, MANAGED_RC):
            invalid = deepcopy(metadata)
            invalid["files"][field] = "changed"
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_staged(invalid)
        metadata.pop("host_service_inputs")
        with self.assertRaises(ValueError):
            validate_staged(metadata)
