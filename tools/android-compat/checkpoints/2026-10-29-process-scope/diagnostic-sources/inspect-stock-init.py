from pathlib import Path
import sys,json,tempfile,base64,urllib.request,concurrent.futures
root=Path.cwd();sys.path.insert(0,str(root/'tools/android-compat'))
from art_inputs import extract_system,Ext4,elf_needed,sha256
work=root/'.build/android-services';out=work/'stock-init-inspection';out.mkdir(exist_ok=False)
base='https://android.googlesource.com/platform/system/core/+/refs/tags/android-13.0.0_r2/'
def fetch(name):
 data=base64.b64decode(urllib.request.urlopen(base+'init/'+name+'?format=TEXT',timeout=40).read())
 dest=out/'sources'/name;dest.parent.mkdir(exist_ok=True);dest.write_bytes(data)
 return {'url':base+'init/'+name,'sha256':sha256(dest)}
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
 sources=dict(zip(['main.cpp','init.cpp','selinux.cpp','property_service.cpp'],pool.map(fetch,['main.cpp','init.cpp','selinux.cpp','property_service.cpp'])))
with tempfile.TemporaryDirectory(dir=out,prefix='system-') as directory:
 image=Path(directory)/'system.ext4'
 sdk=extract_system(Path('/Users/petitstrawberry/Development/Rust/Scarlet/.build/android-art-local/arm64-v8a-33_r02.zip'),image)
 system=Ext4(image)
 for name in ['system/bin/init','system/etc/init/hw/init.rc','system/etc/init/hw/init.zygote64.rc','system/etc/classpaths/systemserverclasspath.pb']:
  system.dump('/'+name,out/name)
 needed=elf_needed(out/'system/bin/init')
print('init direct dependencies:',needed)
(out/'inspection.json').write_text(json.dumps({'sdk':sdk,'sources':sources,'init_needed':needed,'files':{str(p.relative_to(out)):sha256(p) for p in out.rglob('*') if p.is_file()}},indent=2)+'\n')
print(out)
