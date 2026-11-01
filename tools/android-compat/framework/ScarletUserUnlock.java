import android.os.IBinder;
import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;
import android.content.Intent;
import android.content.ComponentName;
import android.app.ActivityManager.RunningAppProcessInfo;

import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.lang.reflect.Method;

/** Requests Android user 0 unlock once the system services accept it. */
public final class ScarletUserUnlock {
    private static final PrintStream out =
            new PrintStream(new FileOutputStream(FileDescriptor.out));

    public static void main(String[] args) throws Exception {
        try {
            Class<?> serviceManagerClass = Class.forName("android.os.ServiceManager");
            addService(serviceManagerClass, "android.service.gatekeeper.IGateKeeperService",
                    new GateKeeper());
            out.println("SCARLET_GATEKEEPER_REGISTERED");
        } catch (Exception error) {
            out.println("SCARLET_GATEKEEPER_REGISTER_RETRY " + error);
        }
        final Class<?> serviceManager = Class.forName("android.os.ServiceManager");
        final Method getService = serviceManager.getMethod("getService", String.class);
        final Method checkService = serviceManager.getMethod("checkService", String.class);
        final Class<?> activityManagerStub = Class.forName("android.app.IActivityManager$Stub");
        final Method asInterface = activityManagerStub.getMethod("asInterface", IBinder.class);
        final Class<?> listenerClass = Class.forName("android.os.IProgressListener");
        final Class<?> storageManagerStub = Class.forName("android.os.storage.IStorageManager$Stub");
        final Method storageAsInterface = storageManagerStub.getMethod("asInterface", IBinder.class);
        final Class<?> packageManagerStub = Class.forName("android.content.pm.IPackageManager$Stub");
        final Method packageAsInterface = packageManagerStub.getMethod("asInterface", IBinder.class);
        final Class<?> userManagerStub = Class.forName("android.os.IUserManager$Stub");
        final Method userAsInterface = userManagerStub.getMethod("asInterface", IBinder.class);
        final Class<?> systemProperties = Class.forName("android.os.SystemProperties");
        final Method getProperty = systemProperties.getMethod("get", String.class);

        Object storageManager = null;
        Method isUserKeyUnlocked = null;
        Object packageManager = null;
        Method resolveIntent = null;
        Method getPackageInfo = null;
        Object userManager = null;
        Method isFrameworkUserUnlocked = null;
        final Intent homeIntent = new Intent(Intent.ACTION_MAIN);
        homeIntent.addCategory(Intent.CATEGORY_HOME);
        final Intent launcherIntent = new Intent(homeIntent);
        launcherIntent.setComponent(new ComponentName("com.android.launcher3",
                "com.android.launcher3.uioverrides.QuickstepLauncher"));
        boolean ceAvailable = false;
        boolean frameworkUnlocked = false;
        boolean bootCompleted = false;
        int bootReadyAttempt = -1;
        int launcherStarts = 0;
        int servicesReadyAttempts = 0;
        boolean requestedUserStart = false;
        for (int attempt = 0; attempt < 360; ++attempt) {
            IBinder binder = (IBinder) getService.invoke(null, "activity");
            Object manager = null;
            if (binder != null) {
                manager = asInterface.invoke(null, binder);
                // SystemUI is launched after AMS has initialized its system-service
                // dependencies. A fixed delay can race that initialization.
                if (!requestedUserStart && attempt % 4 == 0) {
                    try {
                        Method running = manager.getClass().getMethod("getRunningAppProcesses");
                        java.util.List<?> processes = (java.util.List<?>) running.invoke(manager);
                        if (processes != null) {
                            for (Object process : processes) {
                                if ("com.android.systemui".equals(
                                        ((RunningAppProcessInfo) process).processName)) {
                                    Method startUser = manager.getClass().getMethod(
                                            "startUserInBackground", int.class);
                                    Object started = startUser.invoke(manager, 0);
                                    requestedUserStart = Boolean.TRUE.equals(started);
                                    out.println("SCARLET_USER_START_AFTER_SYSTEMUI attempt="
                                            + attempt + " started=" + started);
                                    break;
                                }
                            }
                        }
                    } catch (Exception error) {
                        if (attempt % 20 == 0) {
                            out.println("SCARLET_USER_START_QUERY_RETRY attempt=" + attempt
                                    + " error=" + error);
                        }
                    }
                }
                if (!frameworkUnlocked && attempt % 4 == 0) {
                    Method unlock = manager.getClass().getMethod(
                            "unlockUser", int.class, byte[].class, byte[].class, listenerClass);
                    try {
                        Object result = unlock.invoke(manager, 0, null, null, null);
                        if (attempt == 0 || attempt % 20 == 0) {
                            out.println("SCARLET_USER_UNLOCK_REQUEST attempt=" + attempt
                                    + " uid=" + android.os.Process.myUid() + " result=" + result);
                        }
                    } catch (Exception error) {
                        if (attempt == 0 || attempt % 20 == 0) {
                            out.println("SCARLET_USER_UNLOCK_REQUEST_RETRY attempt=" + attempt
                                    + " error=" + error);
                        }
                    }
                }
            }

            if (storageManager == null) {
                IBinder storageBinder = (IBinder) getService.invoke(null, "mount");
                if (storageBinder != null) {
                    storageManager = storageAsInterface.invoke(null, storageBinder);
                    isUserKeyUnlocked = storageManager.getClass().getMethod(
                            "isUserKeyUnlocked", int.class);
                }
            }

            if (storageManager != null) {
                try {
                    Object unlocked = isUserKeyUnlocked.invoke(storageManager, 0);
                    if (Boolean.TRUE.equals(unlocked) && !ceAvailable) {
                        ceAvailable = true;
                        out.println("SCARLET_USER_UNLOCK_DONE attempt=" + attempt
                                + " ce_available=" + systemProperties.getMethod("get", String.class)
                                        .invoke(null, "sys.user.0.ce_available"));
                    }
                } catch (Exception error) {
                    if (attempt == 0 || attempt % 20 == 0) {
                        out.println("SCARLET_USER_UNLOCK_QUERY_RETRY attempt=" + attempt
                                + " error=" + error);
                    }
                }
            }

            if (packageManager == null) {
                IBinder packageBinder = (IBinder) getService.invoke(null, "package");
                if (packageBinder != null) {
                    packageManager = packageAsInterface.invoke(null, packageBinder);
                    resolveIntent = packageManager.getClass().getMethod(
                            "resolveIntent", Intent.class, String.class, long.class, int.class);
                    getPackageInfo = packageManager.getClass().getMethod(
                            "getPackageInfo", String.class, long.class, int.class);
                }
            }
            if (userManager == null) {
                IBinder userBinder = (IBinder) getService.invoke(null, "user");
                if (userBinder != null) {
                    userManager = userAsInterface.invoke(null, userBinder);
                    isFrameworkUserUnlocked = userManager.getClass().getMethod(
                            "isUserUnlocked", int.class);
                }
            }
            frameworkUnlocked = false;
            try {
                bootCompleted = "1".equals(getProperty.invoke(null, "sys.boot_completed"));
            } catch (Exception ignored) {
                bootCompleted = false;
            }
            if (userManager != null) {
                try {
                    frameworkUnlocked = Boolean.TRUE.equals(
                            isFrameworkUserUnlocked.invoke(userManager, 0));
                } catch (Exception error) {
                    if (attempt % 20 == 0) {
                        out.println("SCARLET_USER_STATE_QUERY_RETRY attempt=" + attempt
                                + " error=" + error);
                    }
                }
            }
            if (bootReadyAttempt < 0 && frameworkUnlocked &&
                    (bootCompleted || attempt >= 80)) {
                bootReadyAttempt = attempt;
            }
            if (ceAvailable && packageManager != null) {
                try {
                    Object resolved = resolveIntent.invoke(
                            packageManager, homeIntent, null, 0L, 0);
                    Object autoResolved = resolveIntent.invoke(
                            packageManager, homeIntent, null, 1049600L, 0);
                    Object launcherResolved = resolveIntent.invoke(
                            packageManager, launcherIntent, null, 0L, 0);
                    if (attempt % 20 == 0) {
                        out.println("SCARLET_USER_HOME_QUERY attempt=" + attempt
                                + " all=" + resolved + " auto=" + autoResolved
                                + " launcher=" + launcherResolved
                                + " package=" + getPackageInfo.invoke(
                                        packageManager, "com.android.launcher3", 0L, 0)
                                + " frameworkUnlocked=" + frameworkUnlocked
                                + " bootCompleted=" + bootCompleted);
                    }
                    IBinder search = (IBinder) checkService.invoke(null, "search");
                    boolean launchServicesReady = search != null && search.pingBinder();
                    for (String name : new String[]{"appwidget", "device_policy", "audio",
                            "window", "display", "statusbar", "wallpaper"}) {
                        IBinder service = (IBinder) checkService.invoke(null, name);
                        launchServicesReady &= service != null && service.pingBinder();
                    }
                    servicesReadyAttempts = launchServicesReady ? servicesReadyAttempts + 1 : 0;
                    if (frameworkUnlocked && bootReadyAttempt >= 0 && launcherResolved != null &&
                            binder != null && servicesReadyAttempts >= 4 && attempt >= bootReadyAttempt &&
                            attempt < bootReadyAttempt + 90 && attempt % 20 == 0) {
                        out.println("SCARLET_LAUNCHER_RESOLVED attempt=" + attempt
                                + " result=" + launcherResolved);
                        Method startHome = manager.getClass().getMethod(
                                "startActivityAsUserWithFeature", Class.forName("android.app.IApplicationThread"),
                                String.class, String.class, Intent.class, String.class, IBinder.class,
                                String.class, int.class, int.class,
                                Class.forName("android.app.ProfilerInfo"), android.os.Bundle.class,
                                int.class);
                        launcherIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                        Object started = startHome.invoke(manager, null, "android", null,
                                launcherIntent, null, null, null, 0, 0, null, null, 0);
                        ++launcherStarts;
                        out.println("SCARLET_LAUNCHER_START attempt=" + attempt
                                + " count=" + launcherStarts + " result=" + started);
                        if (launcherStarts >= 3) return;
                    }
                } catch (Exception error) {
                    if (attempt % 20 == 0) {
                        out.println("SCARLET_USER_HOME_QUERY_RETRY attempt=" + attempt
                                + " error=" + error);
                    }
                }
            }
            Thread.sleep(500);
        }
        out.println("SCARLET_USER_UNLOCK_FAIL attempts=360");
        System.exit(1);
    }

    private static void addService(Class<?> serviceManager, String name, IBinder binder)
            throws Exception {
        for (Method method : serviceManager.getMethods()) {
            Class<?>[] parameters = method.getParameterTypes();
            if (!method.getName().equals("addService") || parameters.length < 2
                    || parameters[0] != String.class || parameters[1] != IBinder.class) {
                continue;
            }
            if (parameters.length == 2) {
                method.invoke(null, name, binder);
            } else if (parameters.length == 4) {
                method.invoke(null, name, binder, false, 0);
            }
            return;
        }
        throw new NoSuchMethodException("ServiceManager.addService");
    }

    private static final class GateKeeper extends Binder {
        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            // No trusted execution environment exists in this host build. The
            // setup-completion notification is sufficient for service startup;
            // credential checks remain unavailable instead of being faked.
            if (reply != null) {
                reply.writeNoException();
                if (code == 4) {
                    reply.writeLong(0L);
                }
            }
            return true;
        }
    }
}
