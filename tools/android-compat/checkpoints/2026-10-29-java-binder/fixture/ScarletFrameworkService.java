import android.os.Binder;
import android.os.Bundle;
import android.os.DeadObjectException;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Process;
import android.os.RemoteException;
import android.os.SystemClock;
import java.io.File;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Three labeled Java processes using stock ServiceManager and Binder JNI. */
public final class ScarletFrameworkService {
    private static final String NAME = "scarlet.compat.service";
    private static final String DESCRIPTOR = "scarlet.compat.JavaService";
    private static final String CALLBACK = DESCRIPTOR + ".Callback";
    private static final int ASYNC_COUNT = 4;
    private static final FileOutputStream console = new FileOutputStream(FileDescriptor.out);
    private static final PrintStream out = new PrintStream(console);
    private static final String PREFIX = "SCARLET_FRAMEWORK_SERVICES_";
    private static void event(String text) {
        try {
            console.write((PREFIX + text + "\n").getBytes("UTF-8"));
        } catch (java.io.IOException error) {
            throw new AssertionError(error);
        }
    }
    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private static void mark(String name) throws Exception {
        require(new File("/tmp/java-service-" + name).createNewFile(), "duplicate barrier " + name);
    }
    private static void await(String name) throws Exception {
        File file = new File("/tmp/java-service-" + name);
        for (int i = 0; i < 1000; ++i) {
            if (file.exists()) return;
            Thread.sleep(10);
        }
        throw new AssertionError("barrier timeout " + name);
    }
    // These framework entry points are hidden from SDK android.jar. Reflection
    // calls the real image classes; no service-manager or Binder implementation
    // is supplied by the fixture.
    private static Object serviceManager() throws Exception {
        Method method = Class.forName("android.os.ServiceManager").getDeclaredMethod("getIServiceManager");
        method.setAccessible(true);
        return method.invoke(null);
    }
    private static IBinder lookup(String name) throws Exception {
        Method method = Class.forName("android.os.IServiceManager").getMethod("checkService", String.class);
        return (IBinder) method.invoke(serviceManager(), name);
    }
    private static void publish(String name, IBinder object) throws Exception {
        Class.forName("android.os.IServiceManager").getMethod("addService", String.class, IBinder.class, boolean.class, int.class)
            .invoke(serviceManager(), name, object, false, 8);
    }
    private static final class Service extends Binder {
        private int nextSequence;
        final CountDownLatch drained = new CountDownLatch(1);
        Service() { attachInterface(null, DESCRIPTOR); }
        @Override protected boolean onTransact(int code, Parcel input, Parcel reply, int flags) throws RemoteException {
            if (code == FIRST_CALL_TRANSACTION + 1) {
                input.enforceInterface(DESCRIPTOR);
                int sequence = input.readInt();
                IBinder callback = input.readStrongBinder();
                require((flags & FLAG_ONEWAY) != 0 && getCallingPid() == 0, "oneway transport identity");
                require(input.dataAvail() == 0 && sequence == nextSequence++, "ordered Java oneway delivery");
                require(callback != null && !(callback instanceof Binder), "remote Java callback object");
                Parcel payload = Parcel.obtain();
                try {
                    payload.writeInterfaceToken(CALLBACK);
                    Bundle bundle = new Bundle();
                    bundle.putInt("sequence", sequence);
                    bundle.putString("message", "callback from owner");
                    payload.writeBundle(bundle);
                    require(callback.transact(FIRST_CALL_TRANSACTION, payload, null, FLAG_ONEWAY), "oneway callback accepted");
                } finally { payload.recycle(); }
                if (nextSequence == ASYNC_COUNT) {
                    event("SERVER_ONEWAY_OK");
                    drained.countDown();
                }
                return true;
            }
            if (code != FIRST_CALL_TRANSACTION) return super.onTransact(code, input, reply, flags);
            input.enforceInterface(DESCRIPTOR);
            int caller = input.readInt();
            int value = input.readInt();
            require(input.dataAvail() == 0 && getCallingPid() == caller && caller != Process.myPid(), "cross-process caller identity");
            reply.writeNoException();
            reply.writeInt(value + 1);
            event("SERVER_CALL_OK");
            return true;
        }
    }
    private static void owner() throws Exception {
        Service service = new Service();
        publish(NAME, service);
        require(lookup(NAME) == service, "owner recovers its registered Java Binder");
        event("REGISTERED");
        mark("ready");
        await("client-done");
        await("denied-done");
        require(service.drained.await(10, TimeUnit.SECONDS), "server oneway completion");
        event("OWNER_EXITING");
    }
    private static void callbacks(IBinder service) throws Exception {
        CountDownLatch received = new CountDownLatch(ASYNC_COUNT);
        Binder callback = new Binder() {
            private int nextSequence;
            @Override protected boolean onTransact(int code, Parcel input, Parcel reply, int flags) throws RemoteException {
                if (code != FIRST_CALL_TRANSACTION) return super.onTransact(code, input, reply, flags);
                input.enforceInterface(CALLBACK);
                Bundle bundle = input.readBundle(ScarletFrameworkService.class.getClassLoader());
                require((flags & FLAG_ONEWAY) != 0 && getCallingPid() == 0, "oneway callback identity");
                require(bundle != null && bundle.getInt("sequence", -1) == nextSequence++
                    && "callback from owner".equals(bundle.getString("message")) && input.dataAvail() == 0,
                    "ordered cross-process Bundle callback");
                received.countDown();
                return true;
            }
        };
        for (int sequence = 0; sequence < ASYNC_COUNT; ++sequence) {
            Parcel payload = Parcel.obtain();
            try {
                payload.writeInterfaceToken(DESCRIPTOR);
                payload.writeInt(sequence);
                payload.writeStrongBinder(callback);
                require(service.transact(IBinder.FIRST_CALL_TRANSACTION + 1, payload, null, IBinder.FLAG_ONEWAY), "oneway request accepted");
            } finally { payload.recycle(); }
        }
        require(received.await(10, TimeUnit.SECONDS), "all Java oneway callbacks");
        event("CLIENT_CALLBACKS_OK");
    }
    private static void client() throws Exception {
        await("ready");
        IBinder object = lookup(NAME);
        require(object != null && !(object instanceof Binder), "remote Java Binder proxy");
        require(DESCRIPTOR.equals(object.getInterfaceDescriptor()), "remote interface descriptor");
        CountDownLatch died = new CountDownLatch(1);
        object.linkToDeath(died::countDown, 0);
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeInt(Process.myPid());
            data.writeInt(41);
            require(object.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0), "remote transact");
            reply.readException();
            require(reply.readInt() == 42 && reply.dataAvail() == 0, "remote Java reply");
            event("CLIENT_CALL_OK");
            callbacks(object);
            mark("client-done");
            require(died.await(10, TimeUnit.SECONDS), "Java DeathRecipient callback");
            require(!object.isBinderAlive(), "dead Binder proxy");
            boolean dead = false;
            try { object.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0); }
            catch (DeadObjectException expected) { dead = true; }
            require(dead, "dead-object exception");
            long deadline = SystemClock.uptimeMillis() + 5000;
            while (lookup(NAME) != null && SystemClock.uptimeMillis() < deadline) Thread.sleep(10);
            require(lookup(NAME) == null, "stock manager removed dead Java service");
            event("DEATH_OK");
        } finally {
            data.recycle();
            reply.recycle();
        }
    }
    private static void denied() throws Exception {
        await("ready");
        require(lookup(NAME) == null, "Shield denied Java findService");
        boolean denied = false;
        try { publish("scarlet.compat.denied", new Service()); }
        catch (InvocationTargetException error) { denied = error.getCause() instanceof SecurityException; }
        require(denied, "Shield denied Java addService");
        event("DENIED_OK");
        mark("denied-done");
    }
    public static void main(String[] args) throws Exception {
        try {
            run(args);
        } catch (Throwable error) {
            event("FAIL " + (args.length == 1 ? args[0] : "unknown"));
            error.printStackTrace(out);
            out.flush();
            if (error instanceof Exception) throw (Exception) error;
            throw new AssertionError(error);
        }
    }
    private static void run(String[] args) throws Exception {
        require(args.length == 1, "one peer role");
        String mode = args[0];
        String expected = mode.equals("owner") ? "u:r:scarlet_publisher:s0" :
            mode.equals("client") ? "u:r:scarlet_client:s0" : "u:r:scarlet_denied:s0";
        byte[] bytes = new byte[128];
        try (FileInputStream input = new FileInputStream("/proc/self/attr/current")) {
            int count = input.read(bytes);
            require(count > 0 && new String(bytes, 0, count, "UTF-8").trim().equals(expected), "native Shield peer label");
        }
        event("PEER " + mode);
        if (mode.equals("owner")) owner();
        else if (mode.equals("client")) client();
        else if (mode.equals("denied")) denied();
        else throw new AssertionError("unknown role");
        event("DONE " + mode);
    }
}
