import android.os.Binder;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Parcel;
import android.os.Process;
import android.os.SystemClock;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/** Framework/JNI probe launched by the stock app_process64 RuntimeInit path. */
public final class ScarletFrameworkHello {
    // RuntimeInit redirects System.out to logd in application mode. Keep this
    // fixture's evidence on the inherited console without replacing its logger.
    private static final PrintStream out = new PrintStream(new FileOutputStream(FileDescriptor.out));

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void checkParcel() {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            Bundle original = new Bundle();
            original.putInt("answer", 42);
            original.putString("message", "Scarlet Android");
            data.writeBundle(original);
            data.setDataPosition(0);
            Bundle decoded = data.readBundle(ScarletFrameworkHello.class.getClassLoader());
            require(decoded.getInt("answer") == 42 && "Scarlet Android".equals(decoded.getString("message")), "Bundle/Parcel round trip");
            data.setDataSize(0);
            data.setDataPosition(0);
            data.writeInt(35);
            Binder local = new Binder() {
                @Override protected boolean onTransact(int code, Parcel input, Parcel output, int flags) {
                    if (code != 1) return false;
                    output.writeNoException();
                    output.writeInt(input.readInt() + 7);
                    return true;
                }
            };
            require(local.transact(1, data, reply, 0), "local Binder dispatch");
            reply.readException();
            require(reply.readInt() == 42, "local Binder reply");
        } catch (Exception error) {
            throw new AssertionError(error);
        } finally {
            data.recycle();
            reply.recycle();
        }
        out.println("SCARLET_FRAMEWORK_PARCEL_BINDER_OK");
    }

    private static void checkLooper() throws Exception {
        HandlerThread thread = new HandlerThread("scarlet-framework");
        thread.start();
        Handler handler = new Handler(thread.getLooper());
        CountDownLatch done = new CountDownLatch(1);
        AtomicInteger state = new AtomicInteger();
        long started = SystemClock.uptimeMillis();
        require(handler.post(() -> {
            state.compareAndSet(0, 1);
            handler.postDelayed(() -> {
                if (state.compareAndSet(1, 2) && SystemClock.uptimeMillis() >= started + 20) done.countDown();
            }, 20);
        }), "Handler post");
        boolean completed = done.await(5, TimeUnit.SECONDS);
        thread.quitSafely();
        thread.join(5000);
        require(completed && state.get() == 2 && !thread.isAlive(), "Looper wake, delayed dispatch and shutdown");
        out.println("SCARLET_FRAMEWORK_LOOPER_OK");
    }

    public static void main(String[] args) throws Exception {
        out.println("SCARLET_FRAMEWORK_BEGIN");
        require(Process.myPid() > 0 && Process.myTid() > 0 && Process.is64Bit(), "native process identity");
        checkParcel();
        checkLooper();
        Thread watchdog = new Thread(() -> {
            try { Thread.sleep(10000); } catch (InterruptedException e) { return; }
            out.println("DIAGNOSTIC_SHUTDOWN_THREADS");
            for (java.util.Map.Entry<Thread,StackTraceElement[]> e : Thread.getAllStackTraces().entrySet()) {
                out.println(e.getKey().getName() + " daemon=" + e.getKey().isDaemon() + " state=" + e.getKey().getState());
                for (StackTraceElement f : e.getValue()) out.println("  " + f);
            }
            Process.sendSignal(Process.myPid(), 3);
        }, "shutdown-diagnostic");
        watchdog.setDaemon(true);
        watchdog.start();
        out.println("SCARLET_FRAMEWORK_OK");
    }
}
