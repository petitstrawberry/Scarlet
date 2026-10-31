#!/usr/bin/env python3
"""Build the bounded libbinder compatibility object for the stock audio HAL."""
import argparse,json,subprocess
from pathlib import Path
from art_inputs import sha256

SOURCE=Path(__file__).resolve().parent
def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--compiler",type=Path,required=True);parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args();output=args.output.resolve()
    if output.exists():parser.error("output must not exist")
    output.mkdir(parents=True);binary=output/"libscarlet-audiobinder.so";source=SOURCE/"audio-binder-compat.cpp"
    command=[str(args.compiler.resolve()),"-std=c++17","-O2","-Wall","-Wextra","-Werror","-fPIC","-shared",str(source),"-ldl","-o",str(binary)]
    subprocess.run(command,check=True)
    record={"kind":"scarlet-audio-binder-compat-v1","result":"BUILT","source_sha256":sha256(source),"binary_sha256":sha256(binary),"commands":[command]}
    (output/"build.json").write_text(json.dumps(record,indent=2)+"\n");print(json.dumps(record,indent=2))
if __name__=="__main__":main()
