from pathlib import Path
import json,sys,tempfile,shutil
root=Path.cwd();sys.path.insert(0,str(root/'tools/android-compat'))
from art_inputs import Ext4,extract_system,verify_art_runtime,sha256,elf_needed
work=root/'.build/android-services';out=work/'stock-init-diagnostic-inputs-v2';out.mkdir(exist_ok=False)
runtime=Path('/Users/petitstrawberry/Development/Rust/Scarlet/.build/android-art-local/runtime');verify_art_runtime(runtime)
with tempfile.TemporaryDirectory(prefix='system-',dir=out) as directory:
 image=Path(directory)/'system.ext4'
 sdk=extract_system(Path('/Users/petitstrawberry/Development/Rust/Scarlet/.build/android-art-local/arm64-v8a-33_r02.zip'),image)
 system=Ext4(image);system.dump('/system/bin/init',out/'system/bin/init')
 system.dump('/system/bin/bootstrap/linker64',out/'system/bin/bootstrap/linker64')
 system.tree('/system/lib64/bootstrap',out/'system/lib64/bootstrap')
 needed={};queue=[out/'system/bin/init'];seen=set()
 while queue:
  binary=queue.pop(0);key=str(binary.relative_to(out));needed[key]=elf_needed(binary)
  for lib in needed[key]:
   if lib in seen:continue
   seen.add(lib)
   if any(p.is_file() for p in runtime.glob('**/'+lib)):continue
   system.dump('/system/lib64/'+lib,out/'system/lib64'/lib)
   queue.append(out/'system/lib64'/lib)
 for name in ['system/etc/init/hw/init.rc','system/etc/init/hw/init.zygote64.rc']:
  system.dump('/'+name,out/name)
manifest={'kind':'stock-init-diagnostic-inputs-v1','result':'PREPARED','sdk':sdk,'runtime_sha256':sha256(runtime/'art-runtime.json'),'needed':needed,'files':{str(p.relative_to(out)):sha256(p) for p in sorted(out.rglob('*')) if p.is_file()}}
(out/'init-inputs.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Prepared',len(manifest['files']),'files for bounded init diagnostic; runtime boot not tested')
