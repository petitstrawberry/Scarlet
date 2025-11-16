Usage: build image and deploy rootfs into your workspace

1) Build the Docker image (run from repo root):

```bash
docker build -t scarlet-dev .
```

docker run --rm -v "$(pwd)":/workspaces/Scarlet scarlet-dev /opt/scripts/deploy_rootfs.sh

2) Run the container with your workspace mounted, then run the deploy script inside the container:

```bash
# This runs the deploy script in a temporary container with workspace mounted
docker run --rm -v "$(pwd)":/workspaces/Scarlet scarlet-dev /opt/scripts/deploy_rootfs.sh
```

The deploy script will extract the prebuilt `rootfs` tarball located at `/opt/prebuilt/linux-riscv64.tar` inside the image into `/workspaces/Scarlet/mkfs/rootfs/system/linux-riscv64/` on the host-mounted workspace.

Optional environment variables:
- `TARGET_UID` and `TARGET_GID`: if set, the script will `chown -R TARGET_UID:TARGET_GID` the deployed files. Useful to preserve host user ownership, e.g.: 

```bash
docker run --rm -v "$(pwd)":/workspaces/Scarlet -e TARGET_UID=$(id -u) -e TARGET_GID=$(id -g) scarlet-dev /opt/scripts/deploy_rootfs.sh
```

Notes:
- Ensure you do not bind-mount `/workspaces/Scarlet` during image build if you expect the deploy script to copy/extract into the host workspace later.
- If you prefer building directly into the mounted workspace, run the Buildroot `make` inside a container with the workspace mounted (slower but immediate).

Notes:
- Do not mount the workspace at build time if you want the built artifacts visible inside the image; the deploy step is intended to run at container runtime when the workspace is mounted.
- If you prefer automatic deploy on container start, you can run the container with a command that invokes `/opt/scripts/deploy_rootfs.sh`.
