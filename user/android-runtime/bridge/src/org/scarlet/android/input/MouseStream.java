package org.scarlet.android.input;

import java.io.IOException;
import java.io.InputStream;

/** Reads complete 40-byte frames and releases all synthetic state on exit or failure. */
final class MouseStream {
    interface Clock {
        long uptimeMillis();
    }

    private final InputStream input;
    private final MouseState state;
    private final MouseState.Sink sink;
    private final Clock clock;
    private final byte[] frame = new byte[MouseProtocol.FRAME_SIZE];

    MouseStream(InputStream input, MouseState state, MouseState.Sink sink, Clock clock) {
        this.input = input;
        this.state = state;
        this.sink = sink;
        this.clock = clock;
    }

    void run() throws IOException {
        int rejected = 0;
        try {
            while (readFrame()) {
                MouseProtocol.Frame decoded = MouseProtocol.decode(frame);
                try {
                    state.apply(decoded, clock.uptimeMillis(), sink);
                } catch (MouseState.RejectedEventException failure) {
                    rejected++;
                    // A focus/window transition can reject one event. Reset the
                    // synthetic state so later navigation input stays usable.
                    try {
                        state.cancel(clock.uptimeMillis(), sink);
                    } catch (MouseState.RejectedEventException ignored) {
                        // cancel() already cleared the state before emitting.
                    }
                }
            }
        } catch (IOException | MouseProtocol.ProtocolException | RuntimeException failure) {
            try {
                state.releaseAll(clock.uptimeMillis(), sink);
            } catch (IOException releaseFailure) {
                failure.addSuppressed(releaseFailure);
            }
            if (failure instanceof IOException) {
                throw (IOException) failure;
            }
            if (failure instanceof RuntimeException) {
                throw (RuntimeException) failure;
            }
            throw new IOException(failure.getMessage(), failure);
        }
        state.releaseAll(clock.uptimeMillis(), sink);
        if (rejected != 0) {
            System.err.println("scarlet-mouse-bridge: recovered from "
                    + rejected + " transient input rejections");
        }
    }

    private boolean readFrame() throws IOException, MouseProtocol.ProtocolException {
        int offset = 0;
        while (offset < frame.length) {
            int count = input.read(frame, offset, frame.length - offset);
            if (count < 0) {
                if (offset == 0) {
                    return false;
                }
                throw new MouseProtocol.ProtocolException(
                        "truncated frame: got " + offset + " of " + frame.length + " bytes");
            }
            if (count == 0) {
                continue;
            }
            offset += count;
        }
        return true;
    }
}

