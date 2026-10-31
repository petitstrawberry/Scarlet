#!/usr/bin/env python3
"""Build the bounded audioserver-to-audio-HAL startup helper."""
import argparse
import json
from pathlib import Path
import subprocess

from art_inputs import sha256

SOURCE=Path(__file__).resolve().parent

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args();output=args.output.resolve()
    if output.exists():parser.error("output must not exist")
    output.mkdir(parents=True);binary=output/"scarlet-audio-late-start";source=SOURCE/"audio-late-start.c"
    command=[str(args.compiler.resolve()),"-std=c11","-O2","-Wall","-Wextra","-Werror",str(source),"-static","-o",str(binary)]
    subprocess.run(command,check=True)
    record={"kind":"scarlet-audio-late-start-v1","result":"BUILT","source_sha256":sha256(source),
            "binary_sha256":sha256(binary),"commands":[command]}
    (output/"build.json").write_text(json.dumps(record,indent=2)+"\n")
    print(json.dumps(record,indent=2))
if __name__=="__main__":main()
