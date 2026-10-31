import android.os.IBinder;
import android.content.Intent;

import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.lang.reflect.Method;

/** Requests Android user 0 unlock once the system services accept it. */
public final class ScarletUserUnlock {
    private static final PrintStream out =
            new PrintStream(new FileOutputStream(FileDescriptor.out));

    public static void main(String[] args) throws Exception {
        final Class<?> serviceManager = Class.forName("android.os.ServiceManager");
        final Method getService = serviceManager.getMethod("getService", String.class);
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
        final Method setProperty = systemProperties.getMethod("set", String.class, String.class);

        Object storageManager = null;
        Method isUserKeyUnlocked = null;
        Object packageManager = null;
        Method resolveIntent = null;
        Method getPackageInfo = null;
        Object userManager = null;
        Method isFrameworkUserUnlocked = null;
        final Intent homeIntent = new Intent(Intent.ACTION_MAIN);
        homeIntent.addCategory(Intent.CATEGORY_HOME);
        final Intent sdksetupIntent = new Intent(homeIntent);
        sdksetupIntent.setPackage("com.android.sdksetup");
        boolean ceAvailable = false;
        boolean restartedUser = false;
        for (int attempt = 0; attempt < 360; ++attempt) {
            IBinder binder = (IBinder) getService.invoke(null, "activity");
            Object manager = null;
            if (binder != null) {
                manager = asInterface.invoke(null, binder);
                if (!ceAvailable) {
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
                        setProperty.invoke(null, "sys.user.0.ce_available", "true");
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
            boolean frameworkUnlocked = false;
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
            if (ceAvailable && !frameworkUnlocked && attempt >= 140 && binder != null) {
                try {
                    if (!restartedUser) {
                        Method startUser = manager.getClass().getMethod(
                                "startUserInBackground", int.class);
                        Object started = startUser.invoke(manager, 0);
                        restartedUser = true;
                        out.println("SCARLET_USER_RESTART attempt=" + attempt
                                + " started=" + started);
                    }
                    if (restartedUser && attempt == 160) {
                        Method unlock = manager.getClass().getMethod(
                                "unlockUser", int.class, byte[].class, byte[].class, listenerClass);
                        Object unlocked = unlock.invoke(manager, 0, null, null, null);
                        out.println("SCARLET_USER_UNLOCK_AFTER_START attempt=" + attempt
                                + " result=" + unlocked);
                    }
                } catch (Exception error) {
                    out.println("SCARLET_USER_RESTART_RETRY attempt=" + attempt
                            + " error=" + error);
                }
            }
            if (ceAvailable && packageManager != null) {
                try {
                    Object resolved = resolveIntent.invoke(
                            packageManager, homeIntent, null, 0L, 0);
                    Object autoResolved = resolveIntent.invoke(
                            packageManager, homeIntent, null, 1049600L, 0);
                    Object sdksetupResolved = resolveIntent.invoke(
                            packageManager, sdksetupIntent, null, 0L, 0);
                    if (attempt % 20 == 0) {
                        out.println("SCARLET_USER_HOME_QUERY attempt=" + attempt
                                + " all=" + resolved + " auto=" + autoResolved
                                + " sdksetup=" + sdksetupResolved
                                + " package=" + getPackageInfo.invoke(
                                        packageManager, "com.android.sdksetup", 0L, 0)
                                + " frameworkUnlocked=" + frameworkUnlocked);
                    }
                    if (resolved != null && binder != null) {
                        out.println("SCARLET_USER_HOME_RESOLVED attempt=" + attempt
                                + " auto=" + autoResolved + " result=" + resolved);
                        Method startUser = manager.getClass().getMethod(
                                "startUserInBackground", int.class);
                        out.println("SCARLET_USER_HOME_START result="
                                + startUser.invoke(manager, 0));
                        return;
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
}
