from copy import deepcopy
import hashlib
import unittest
import xml.etree.ElementTree as ET

from vintf_inputs import DEVICE, FRAMEWORK, build_manifests, validate_selection, validate_staged

SOURCE = b'''<manifest version="1.0" type="framework">
<hal><name>android.frameworks.stats</name><version>1.0</version><fqname>@1.0::IStats/default</fqname></hal>
<hal format="aidl"><name>android.frameworks.stats</name><version>1</version><fqname>IStats/default</fqname><fqname>IStats/other</fqname></hal>
<hal format="aidl"><name>android.system.suspend</name><version>1</version><fqname>ISystemSuspend/default</fqname></hal>
</manifest>'''
SELECTION = {"framework": ["android.frameworks.stats.IStats/default"], "device": []}


class VintfInputsTest(unittest.TestCase):
    def test_selects_one_exact_stock_contract_and_no_device_hal(self):
        outputs, proof = build_manifests(SELECTION, {"stock.xml": SOURCE})
        framework = ET.fromstring(outputs[FRAMEWORK])
        self.assertEqual(len(framework.findall("hal")), 1)
        hal = framework.find("hal")
        self.assertEqual(hal.get("format"), "aidl")
        self.assertEqual(hal.findtext("name"), "android.frameworks.stats")
        self.assertEqual(hal.findtext("version"), "1")
        self.assertEqual([x.text for x in hal.findall("fqname")], ["IStats/default"])
        self.assertEqual(ET.fromstring(outputs[DEVICE]).findall("hal"), [])
        self.assertEqual(proof["contracts"][SELECTION["framework"][0]]["source_sha256"], hashlib.sha256(SOURCE).hexdigest())

    def test_rejects_unknown_contracts_device_hal_claims_and_ambiguous_sources(self):
        selection = deepcopy(SELECTION)
        selection["framework"] = ["android.frameworks.stats.IStats/missing"]
        with self.assertRaisesRegex(ValueError, "absent"):
            build_manifests(selection, {"stock.xml": SOURCE})
        with self.assertRaisesRegex(ValueError, "duplicate"):
            build_manifests(SELECTION, {"first.xml": SOURCE, "second.xml": SOURCE})
        selection["device"] = ["android.hardware.power.IPower/default"]
        with self.assertRaisesRegex(ValueError, "provider"):
            validate_selection(selection)
        selection = {"framework": ["bad\nXML"], "device": []}
        with self.assertRaises(ValueError):
            validate_selection(selection)

    def test_staging_requires_proof_and_rejects_stale_or_extra_inventory(self):
        outputs, proof = build_manifests(SELECTION, {"stock.xml": SOURCE})
        metadata = {"profile": {"vintf": SELECTION}, "vintf_inputs": proof,
                    "files": {name: hashlib.sha256(data).hexdigest() for name, data in outputs.items()}}
        validate_staged(metadata)
        for change in ("missing-proof", "changed-selection", "extra-device-manifest", "changed-output"):
            bad = deepcopy(metadata)
            if change == "missing-proof": bad.pop("vintf_inputs")
            elif change == "changed-selection": bad["profile"]["vintf"]["framework"] = []
            elif change == "extra-device-manifest": bad["files"]["vendor/etc/vintf/manifest/extra.xml"] = "digest"
            else: bad["files"][FRAMEWORK] = "changed"
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_staged(bad)
