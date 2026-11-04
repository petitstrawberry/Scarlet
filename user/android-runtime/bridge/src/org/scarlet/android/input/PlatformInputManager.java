package org.scarlet.android.input;

import android.hardware.input.InputManager;
import android.view.MotionEvent;

import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

/** Cached reflection around the stock Android 13 InputManager and MotionEvent APIs. */
final class PlatformInputManager {
    private static final long RETRY_MILLIS = 100;

    private static final Method WAIT_FOR_SERVICE;
    private static final Method GET_INSTANCE;
    static final Method INJECT_INPUT_EVENT;
    static final Method SET_ACTION_BUTTON;
    static final Method SET_DISPLAY_ID;
    static final Method SET_FILE_DESCRIPTOR;

    static {
        try {
            Method waitForService = Class.forName("android.os.ServiceManager")
                    .getDeclaredMethod("waitForService", String.class);
            Method getInstance = InputManager.class.getDeclaredMethod("getInstance");
            Method inject = InputManager.class.getDeclaredMethod(
                    "injectInputEvent", android.view.InputEvent.class, int.class);
            Method setActionButton = MotionEvent.class.getDeclaredMethod(
                    "setActionButton", int.class);
            Method setDisplayId = MotionEvent.class.getDeclaredMethod(
                    "setDisplayId", int.class);
            Method setFileDescriptor = java.io.FileDescriptor.class.getDeclaredMethod(
                    "setInt$", int.class);
            waitForService.setAccessible(true);
            getInstance.setAccessible(true);
            inject.setAccessible(true);
            setActionButton.setAccessible(true);
            setDisplayId.setAccessible(true);
            setFileDescriptor.setAccessible(true);
            WAIT_FOR_SERVICE = waitForService;
            GET_INSTANCE = getInstance;
            INJECT_INPUT_EVENT = inject;
            SET_ACTION_BUTTON = setActionButton;
            SET_DISPLAY_ID = setDisplayId;
            SET_FILE_DESCRIPTOR = setFileDescriptor;
        } catch (ReflectiveOperationException failure) {
            throw new ExceptionInInitializerError(failure);
        }
    }

    private PlatformInputManager() {}

    static InputManager awaitService() throws IOException {
        for (;;) {
            try {
                Object service = WAIT_FOR_SERVICE.invoke(null, "input");
                if (service != null) {
                    Object manager = GET_INSTANCE.invoke(null);
                    if (manager != null) {
                        return (InputManager) manager;
                    }
                }
            } catch (InvocationTargetException failure) {
                Throwable cause = failure.getCause();
                if (!isServiceNotReady(cause)) {
                    throw reflectionFailure("obtaining stock InputManager", cause);
                }
            } catch (IllegalAccessException failure) {
                throw reflectionFailure("accessing stock InputManager", failure);
            }
            try {
                Thread.sleep(RETRY_MILLIS);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                throw new IOException("interrupted while waiting for stock input service", interrupted);
            }
        }
    }

    static boolean isServiceNotReady(Throwable failure) {
        if (failure == null) {
            return true;
        }
        // InputManager wraps ServiceNotFoundException in IllegalStateException.
        // Only retry that actual cause; unrelated initialization errors remain
        // visible instead of turning into an indefinite readiness wait.
        for (int depth = 0; failure != null && depth < 16; depth++) {
            if (failure.getClass().getName().endsWith("$ServiceNotFoundException")) {
                return true;
            }
            failure = failure.getCause();
        }
        return false;
    }

    static IOException reflectionFailure(String operation, Throwable failure) {
        return new IOException("failed " + operation + ": " + failure, failure);
    }
}

