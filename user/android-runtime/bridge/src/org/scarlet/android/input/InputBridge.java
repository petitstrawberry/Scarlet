package org.scarlet.android.input;

import android.system.Os;
import android.system.ErrnoException;
import android.os.SystemClock;

import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.lang.reflect.InvocationTargetException;

/** app_process entrypoint for the inherited-fd3 absolute mouse bridge. */
public final class InputBridge {
    private static final int INPUT_FD = 3;

    private InputBridge() {}

    public static void main(String[] args) {
        if (args.length != 0) {
            System.err.println("scarlet-mouse-bridge: no arguments are accepted; input is fd 3");
            System.exit(2);
        }
        try {
            run();
        } catch (Throwable failure) {
            System.err.println("scarlet-mouse-bridge: " + failure.getMessage());
            failure.printStackTrace(System.err);
            System.exit(1);
        }
    }

    static void run() throws IOException {
        AndroidMouseSink sink = new AndroidMouseSink(PlatformInputManager.awaitService());
        System.err.println("SCARLET_ANDROID_INPUT_BRIDGE_READY");
        FileDescriptor inheritedFd = inheritedFd(INPUT_FD);
        try (InputStream input = new FileInputStream(inheritedFd)) {
            MouseStream stream = new MouseStream(
                    input, new MouseState(), sink, SystemClock::uptimeMillis);
            stream.run();
        } finally {
            // Android's public FileInputStream(FileDescriptor) constructor is
            // non-owning, so close the inherited descriptor exactly once here.
            try {
                Os.close(inheritedFd);
            } catch (ErrnoException failure) {
                throw new IOException("cannot close inherited fd 3: " + failure.getMessage(), failure);
            }
        }
    }

    private static FileDescriptor inheritedFd(int rawFd) throws IOException {
        FileDescriptor inherited = new FileDescriptor();
        try {
            PlatformInputManager.SET_FILE_DESCRIPTOR.invoke(inherited, rawFd);
            return inherited;
        } catch (IllegalAccessException failure) {
            throw PlatformInputManager.reflectionFailure(
                    "accessing FileDescriptor.setInt$", failure);
        } catch (InvocationTargetException failure) {
            throw PlatformInputManager.reflectionFailure(
                    "wrapping inherited fd 3", failure.getCause());
        }
    }
}

