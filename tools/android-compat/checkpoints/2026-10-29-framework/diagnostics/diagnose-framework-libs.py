from pathlib import Path
import hashlib, importlib.util, json, shutil, sys
root=Path.cwd();sys.path.insert(0,str(root/'tools/android-compat'))
from art_inputs import Ext4,sha256
from stock_art import derive_cpu_abi_properties
spec=importlib.util.spec_from_file_location('runner',root/'tools/android-compat/run-direct.py')
runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
inputs=root/'.build/android-services/framework-inputs';system=Ext4(inputs/'system.ext4')
for name in ['telephony-common.jar','voip-common.jar','ims-common.jar']:
 target=inputs/'system/framework'/name
 if not target.exists():system.dump('/system/framework/'+name,target)
import runpy
classpath=runpy.run_path(str(root/'.build/android-services/classpath-diagnostic.py'))['boot_classpath'](inputs)
# Diagnostic classpath derived from the image fragments.
core=['/apex/com.android.art/javalib/core-oj.jar','/apex/com.android.art/javalib/core-libart.jar']
platform=['framework.jar','framework-graphics.jar','ext.jar','telephony-common.jar','voip-common.jar','ims-common.jar']
boot=classpath
launch={'argv':['/system/bin/scarlet-program-probe','-Xint','-Xusejit:false','-Ximage:/system/nonexistent/boot.art','-Xnoimage-dex2oat','-Xms16m','-Xmx256m','/system/bin','--application','ScarletArtHello'],
 'env':['PATH=/system/bin:/apex/com.android.art/bin','ANDROID_ROOT=/system','ANDROID_DATA=/data','ANDROID_ART_ROOT=/apex/com.android.art','ANDROID_I18N_ROOT=/apex/com.android.i18n','ANDROID_TZDATA_ROOT=/apex/com.android.tzdata','ANDROID_LOG_TAGS=*:v','BOOTCLASSPATH='+':'.join(boot),'CLASSPATH=/data/local/tmp/classes.dex','LD_PRELOAD=/system/lib64/libprobe-stderr-logger.so']}
stage,run=runner.stage_stock_inputs,runner.subprocess.run

def staging(stock,runtime,tree):
 result=stage(stock,runtime,tree)
 logger=Path("/Users/petitstrawberry/.codex/worktrees/shv-pr-validation/Scarlet/.build/android-services/libframework-exit-logger.so")
 shutil.copy2(logger,tree/"system/lib64/libprobe-stderr-logger.so")
 for jar in boot:
  source=inputs/jar.lstrip('/');dest=tree/jar.lstrip('/')
  dest.parent.mkdir(parents=True,exist_ok=True)
  if dest.exists():assert sha256(dest)==sha256(source)
  else:shutil.copy2(source,dest)
 for source in sorted((inputs/'apex/com.android.tethering/lib64').rglob('*')):
  dest=tree/source.relative_to(inputs)
  if source.is_dir():dest.mkdir(parents=True,exist_ok=True)
  elif source.is_symlink():
   if not dest.is_symlink():dest.symlink_to(source.readlink())
   else:assert dest.readlink()==source.readlink()
  elif dest.exists():assert sha256(dest)==sha256(source)
  else:shutil.copy2(source,dest)
 prop=tree/'data/local/tmp/property-values.prop'
 values=dict(line.split('=',1) for line in prop.read_text().splitlines() if line and not line.startswith('#'))
 initialized=derive_cpu_abi_properties(values)
 initialized["dalvik.vm.usejit"]="false"
 prop.write_text(''.join(f'{key}={value}\n' for key,value in sorted(initialized.items())))
 dex=Path('/Users/petitstrawberry/Development/Rust/Scarlet/.build/android-art-local/dex/classes.dex')
 shutil.copy2(dex,tree/'data/local/tmp/classes.dex')
 evidence={'purpose':'stock app_process startup diagnostic; does not certify framework services or APK execution',
  'boot_properties_sha256':sha256(prop),'derived_properties':{k:v for k,v in initialized.items() if values.get(k)!=v},'diagnostic_preload_sha256':sha256(logger),'sdk':json.loads((inputs/'sdk.json').read_text()),'jar_sha256':{p:sha256(inputs/p.lstrip('/')) for p in boot},'tethering':json.loads((inputs/'tethering-libraries.json').read_text()),'dex_sha256':sha256(dex),'launch':launch}
 (tree.parents[1].parent/'framework-diagnostic.json').write_text(json.dumps(evidence,indent=2)+'\n')
 return result

def build(command,*args,**kwargs):
 if command[0]=='cargo' and 'env' in kwargs:
  env=kwargs['env']=kwargs['env'].copy()
  env['SCARLET_ART_ARGS']='\x1f'.join(launch['argv']);env['SCARLET_ART_ENV']='\x1f'.join(launch['env'])
  output=Path(command[command.index('--target-dir')+1]).parent
  (output/'program-launch.json').write_text(json.dumps(launch,indent=2)+'\n')
 return run(command,*args,**kwargs)
runner.stage_stock_inputs,runner.subprocess.run=staging,build
raise SystemExit(runner.main())
