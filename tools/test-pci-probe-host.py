#!/usr/bin/env python3
"""Compile the actual PCI discovery/retry methods against dependency fixtures."""
from pathlib import Path
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parents[1]
source = (ROOT/'kernel/src/device/manager.rs').read_text()
def method(name):
    start = source.index('    '+('pub ' if name=='probe_pci_devices' else '')+'fn '+name+'(')
    brace = source.index('{',start); depth=1; end=brace+1
    while depth:
        depth += (source[end]=='{')-(source[end]=='}'); end+=1
    return source[start:end]
fixture = r'''
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}};
struct IrqSpinLock<T>(Mutex<T>);
impl<T> IrqSpinLock<T> { fn new(t:T)->Self {Self(Mutex::new(t))} fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()} }
trait DeviceInfo { fn name(&self)->&str; }
trait DeviceDriver:Send+Sync { fn name(&self)->&str; fn probe(&self,d:&dyn DeviceInfo)->Result<(),&'static str>; }
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord)] enum DriverPriority { Standard,Late }
impl DriverPriority { fn all()->&'static [Self] { &[Self::Standard,Self::Late] } }
fn is_probe_defer(s:&str)->bool {s=="probe: deferred"}
struct DeferredPciDevice {device:Arc<dyn DeviceInfo+Send+Sync>,driver:Arc<dyn DeviceDriver>}
struct DeviceManager {
 discovered_pci_devices:IrqSpinLock<Vec<Arc<dyn DeviceInfo+Send+Sync>>>,
 deferred_pci_devices:IrqSpinLock<Vec<DeferredPciDevice>>,
 bound_pci_devices:IrqSpinLock<Vec<Arc<dyn DeviceInfo+Send+Sync>>>,
 drivers:IrqSpinLock<BTreeMap<DriverPriority,Vec<Arc<dyn DeviceDriver>>>>,
}
impl DeviceManager {
 METHODS
}
struct Device; impl DeviceInfo for Device {fn name(&self)->&str {"radio"}}
struct Driver {calls:AtomicUsize, defers:usize, hard_failure:bool}
impl DeviceDriver for Driver {
 fn name(&self)->&str {"firmware"}
 fn probe(&self,_:&dyn DeviceInfo)->Result<(),&'static str> {
  let n=self.calls.fetch_add(1,Ordering::SeqCst);
  if self.hard_failure {Err("wrong device")} else if n<self.defers {Err("probe: deferred")} else {Ok(())}
 }
}
fn setup(defers:usize)->(DeviceManager,Arc<Driver>,Arc<Driver>) {
 let first=Arc::new(Driver{calls:AtomicUsize::new(0),defers,hard_failure:false});
 let later=Arc::new(Driver{calls:AtomicUsize::new(0),defers:0,hard_failure:false});
 let d:BTreeMap<_,Vec<Arc<dyn DeviceDriver>>>=BTreeMap::from([(DriverPriority::Standard,vec![first.clone() as Arc<dyn DeviceDriver>]),(DriverPriority::Late,vec![later.clone() as Arc<dyn DeviceDriver>])]);
 (DeviceManager{discovered_pci_devices:IrqSpinLock::new(vec![Arc::new(Device)]),deferred_pci_devices:IrqSpinLock::new(vec![]),bound_pci_devices:IrqSpinLock::new(vec![]),drivers:IrqSpinLock::new(d)},first,later)
}
#[test] fn deferred_pci_firmware_retries_after_dependency_and_binds_once_across_priorities() {
 let (m,first,later)=setup(1);m.probe_pci_devices();
 assert_eq!(m.deferred_pci_devices.lock().len(),1); assert_eq!(later.calls.load(Ordering::SeqCst),0);
 m.probe_pci_devices();assert_eq!(first.calls.load(Ordering::SeqCst),1);
 m.retry_deferred_pci_probes();assert_eq!(m.bound_pci_devices.lock().len(),1);
 m.retry_deferred_pci_probes();m.probe_pci_devices();assert_eq!(first.calls.load(Ordering::SeqCst),2);assert_eq!(later.calls.load(Ordering::SeqCst),0);
}
#[test] fn repeated_dependency_deferral_is_retained_without_duplicate_queue_entries() {
 let (m,first,_)=setup(3);m.probe_pci_devices();
 for _ in 0..2 {m.retry_deferred_pci_probes();m.probe_pci_devices();assert_eq!(m.deferred_pci_devices.lock().len(),1);}
 m.retry_deferred_pci_probes();assert_eq!(first.calls.load(Ordering::SeqCst),4);assert!(m.deferred_pci_devices.lock().is_empty());
}
#[test] fn hard_failure_does_not_queue_or_prevent_a_different_driver_matching() {
 let (m,_,later)=setup(0);
 m.drivers.lock().get_mut(&DriverPriority::Standard).unwrap()[0]=Arc::new(Driver{calls:AtomicUsize::new(0),defers:0,hard_failure:true});
 m.probe_pci_devices();m.probe_pci_devices();assert!(m.deferred_pci_devices.lock().is_empty());assert_eq!(later.calls.load(Ordering::SeqCst),1);
}
'''.replace('METHODS',method('probe_pci_devices')+'\n'+method('retry_deferred_pci_probes'))
assert 'self.retry_deferred_pci_probes();' in source.split('pub fn retry_deferred_probes(&self)',1)[1].split('\n    }',1)[0], 'FS retry entry point must include PCI'
out=ROOT/'.build/pci-probe-host';out.mkdir(parents=True,exist_ok=True)
(out/'fixture.rs').write_text(fixture)
with tempfile.TemporaryDirectory(prefix='pci-probe-') as tmp:
    binary=Path(tmp)/'tests'
    with (out/'result.log').open('w') as log:
        subprocess.run(['rustc','--edition=2024','--test',str(out/'fixture.rs'),'-o',str(binary)],stdout=log,stderr=subprocess.STDOUT,check=True)
        subprocess.run([str(binary)],stdout=log,stderr=subprocess.STDOUT,check=True)
print('PCI_PROBE_HOST PASS (3 dependency/ownership regressions)')
