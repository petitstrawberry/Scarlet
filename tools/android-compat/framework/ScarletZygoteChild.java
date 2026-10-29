import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import android.os.Process;
import android.system.Os;
import java.io.DataOutputStream;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;

/** Boot-classpath fixture entrypoint reached through stock Zygote specialization. */
public final class ScarletZygoteChild {
    public static void main(String[] args) throws Exception {
        int uid = Process.myUid(), gid = Os.getgid(), pid = Process.myPid();
        if (uid != 10001 || gid != 10001) throw new IllegalStateException("child identity");
        byte[] bytes = new byte[128];
        int count;
        try (FileInputStream context = new FileInputStream("/proc/self/attr/current")) {
            count = context.read(bytes);
        }
        String label = new String(bytes, 0, count, StandardCharsets.UTF_8).replace("\u0000", "");
        if (!label.equals("u:r:untrusted_app:s0:c1,c256,c512,c768"))
            throw new IllegalStateException("child label");
        try (LocalSocket socket = new LocalSocket()) {
            socket.connect(new LocalSocketAddress("/dev/socket/scarlet_zygote_result", LocalSocketAddress.Namespace.FILESYSTEM));
            DataOutputStream out = new DataOutputStream(socket.getOutputStream());
            byte[] encoded = label.getBytes(StandardCharsets.UTF_8);
            out.writeInt(pid); out.writeInt(uid); out.writeInt(gid);
            out.writeInt(encoded.length); out.write(encoded); out.flush();
            if (socket.getInputStream().read() != 1) throw new IllegalStateException("supervisor acknowledgement");
        }
        System.exit(0);
    }
}
