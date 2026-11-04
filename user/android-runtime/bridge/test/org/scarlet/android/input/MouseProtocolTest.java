package org.scarlet.android.input;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public final class MouseProtocolTest {
    private MouseProtocolTest() {}

    public static void main(String[] args) throws Exception {
        decodesEveryFieldAndSignedCoordinate();
        rejectsMalformedFrames();
        emitsHoverThenHeldMoveAndCompletePrimaryClick();
        supportsSecondaryAndMiddleButtons();
        mapsScrollSidesAndModifiers();
        cancelsGestureAndExitsHover();
        exitsHoverAtEof();
        releasesAllButtonStateAtEof();
        releasesAllButtonStateOnTruncatedFrame();
        System.out.println("MouseProtocolTest: PASS");
    }

    private static void decodesEveryFieldAndSignedCoordinate() throws Exception {
        byte[] raw = encodeFrame(
                MouseProtocol.KIND_POSITION, -17, 42, 0, 0, 123_456_789L, 0x55);
        MouseProtocol.Frame frame = MouseProtocol.decode(raw);
        check(frame.kind == MouseProtocol.KIND_POSITION, "kind");
        check(frame.x == -17 && frame.y == 42, "signed coordinates");
        check(frame.timestampNs == 123_456_789L, "timestamp");
        check(frame.modifiers == 0x55, "modifiers");
    }

    private static void rejectsMalformedFrames() throws Exception {
        byte[] valid = encodeFrame(
                MouseProtocol.KIND_BUTTON, 1, 2, MouseProtocol.BTN_LEFT, 1, 0, 0);
        expectProtocolError(valid, raw -> raw[0] = 0, "magic");
        expectProtocolError(valid, raw -> raw[4] = 2, "version");
        expectProtocolError(valid, raw -> raw[6] = 9, "kind");
        expectProtocolError(valid, raw -> raw[16] = 0, "button code");
        expectProtocolError(valid, raw -> raw[20] = 2, "button value");
        expectProtocolError(valid, raw -> raw[32] = (byte) 0x80, "modifier mask");
        expectProtocolError(valid, raw -> raw[36] = 1, "reserved");
        expectProtocolError(valid, raw -> putInt(raw, 8, 1_000_001), "coordinate bound");
        expectProtocolError(valid, raw -> raw[31] = (byte) 0x80, "unsigned timestamp");

        byte[] scroll = encodeFrame(
                MouseProtocol.KIND_SCROLL, 1, 2, MouseProtocol.REL_WHEEL, 1, 0, 0);
        expectProtocolError(scroll, raw -> putInt(raw, 20, 0), "zero scroll");
        expectProtocolError(scroll, raw -> putInt(raw, 20, 1_001), "large scroll");
        expectProtocolError(scroll, raw -> putInt(raw, 16, 7), "scroll code");

        try {
            MouseProtocol.decode(Arrays.copyOf(valid, MouseProtocol.FRAME_SIZE - 1));
            throw new AssertionError("truncated frame accepted");
        } catch (MouseProtocol.ProtocolException expected) {
            // Expected.
        }
    }

    private static void emitsHoverThenHeldMoveAndCompletePrimaryClick() throws Exception {
        List<MouseState.Event> events = new ArrayList<>();
        MouseState state = new MouseState();
        state.apply(position(10, 20, ms(10), 0), 10, events::add);
        state.apply(position(11, 21, 0, 0), 11, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_LEFT, 1, 11, 21, ms(12)), 12, events::add);
        state.apply(position(12, 22, 0, 0), 13, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_LEFT, 0, 12, 22, ms(14)), 14, events::add);

        expectActions(events,
                MouseState.ACTION_HOVER_ENTER, MouseState.ACTION_HOVER_MOVE,
                MouseState.ACTION_DOWN, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_MOVE,
                MouseState.ACTION_BUTTON_RELEASE, MouseState.ACTION_UP);
        check(events.get(2).buttonState == MouseState.BUTTON_PRIMARY, "down button state");
        check(events.get(3).actionButton == MouseState.BUTTON_PRIMARY, "press action button");
        check(events.get(4).buttonState == MouseState.BUTTON_PRIMARY, "move button state");
        check(events.get(5).buttonState == 0, "release button state is post-release");
        check(events.get(5).actionButton == MouseState.BUTTON_PRIMARY, "release action button");
        check(events.get(6).buttonState == 0 && events.get(6).actionButton == 0, "final up");
        check(events.get(2).downTimeMs == 12 && events.get(6).downTimeMs == 12, "click down time");
    }

    private static void supportsSecondaryAndMiddleButtons() throws Exception {
        List<MouseState.Event> events = new ArrayList<>();
        MouseState state = new MouseState();
        state.apply(buttonFrame(MouseProtocol.BTN_RIGHT, 1, 4, 5, ms(10)), 10, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_MIDDLE, 1, 4, 5, ms(11)), 11, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_RIGHT, 0, 4, 5, ms(12)), 12, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_MIDDLE, 0, 4, 5, ms(13)), 13, events::add);
        expectActions(events,
                MouseState.ACTION_DOWN, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_MOVE, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_BUTTON_RELEASE,
                MouseState.ACTION_BUTTON_RELEASE, MouseState.ACTION_UP);
        check(events.get(1).actionButton == MouseState.BUTTON_SECONDARY, "secondary");
        check(events.get(3).actionButton == MouseState.BUTTON_TERTIARY, "middle");
        check(events.get(4).buttonState == MouseState.BUTTON_TERTIARY,
                "release removes only the released button");
        check(events.get(5).buttonState == 0, "last release has no held buttons");
    }

    private static void mapsScrollSidesAndModifiers() throws Exception {
        List<MouseState.Event> events = new ArrayList<>();
        MouseState state = new MouseState();
        state.apply(scroll(MouseProtocol.REL_WHEEL, -3, 7, 9, 0x01 | 0x08), 10, events::add);
        state.apply(scroll(MouseProtocol.REL_HWHEEL, 2, 8, 9, 0x04), 11, events::add);
        check(events.get(0).action == MouseState.ACTION_SCROLL, "vertical action");
        check(events.get(0).scrollAxis == MouseState.SCROLL_VERTICAL, "vertical axis");
        check(events.get(0).scrollValue == -3f, "vertical value");
        check(events.get(0).metaState == 0x00010001, "shift/meta");
        check(events.get(1).scrollAxis == MouseState.SCROLL_HORIZONTAL, "horizontal axis");
        check(events.get(1).scrollValue == 2f, "horizontal value");
        check(events.get(1).metaState == 0x00000002, "alt");
        check(events.get(0).eventTimeMs == 10, "zero timestamp uses uptime");
        check(MouseState.androidMetaState(0x7f) == 0x00711003, "complete meta mapping");
    }

    private static void cancelsGestureAndExitsHover() throws Exception {
        List<MouseState.Event> events = new ArrayList<>();
        MouseState state = new MouseState();
        state.apply(position(3, 4, ms(10), 0), 10, events::add);
        state.apply(buttonFrame(MouseProtocol.BTN_LEFT, 1, 3, 4, ms(11)), 11, events::add);
        state.apply(cancel(3, 4, ms(12)), 12, events::add);
        expectActions(events,
                MouseState.ACTION_HOVER_ENTER,
                MouseState.ACTION_DOWN, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_CANCEL, MouseState.ACTION_HOVER_EXIT);
        state.releaseAll(13, events::add);
        check(events.size() == 5, "cancel clears all state");
    }

    private static void exitsHoverAtEof() throws Exception {
        RecordingSink sink = new RecordingSink();
        MouseStream stream = new MouseStream(
                new ByteArrayInputStream(positionBytes(9, 10, ms(20), 0)),
                new MouseState(), sink, () -> 21);
        stream.run();
        expectActions(sink.events, MouseState.ACTION_HOVER_ENTER, MouseState.ACTION_HOVER_EXIT);
    }

    private static void releasesAllButtonStateAtEof() throws Exception {
        byte[] primaryDown = button(MouseProtocol.BTN_LEFT, 1, 5, 6, ms(20));
        byte[] middleDown = button(MouseProtocol.BTN_MIDDLE, 1, 5, 6, ms(21));
        RecordingSink sink = new RecordingSink();
        MouseStream stream = new MouseStream(
                new ByteArrayInputStream(concat(primaryDown, middleDown)),
                new MouseState(), sink, () -> 30);
        stream.run();
        expectActions(sink.events,
                MouseState.ACTION_DOWN, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_MOVE, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_BUTTON_RELEASE, MouseState.ACTION_BUTTON_RELEASE,
                MouseState.ACTION_UP);
        check(sink.events.get(4).actionButton == MouseState.BUTTON_PRIMARY, "EOF primary release");
        check(sink.events.get(4).buttonState == MouseState.BUTTON_TERTIARY,
                "EOF primary release is post-release");
        check(sink.events.get(5).actionButton == MouseState.BUTTON_TERTIARY, "EOF middle release");
        check(sink.events.get(5).buttonState == 0, "EOF middle release is post-release");
        check(sink.events.get(6).buttonState == 0, "EOF final up");
    }

    private static void releasesAllButtonStateOnTruncatedFrame() throws Exception {
        byte[] primaryDown = button(MouseProtocol.BTN_LEFT, 1, 5, 6, ms(20));
        byte[] truncated = Arrays.copyOf(positionBytes(7, 8, ms(21), 0), 17);
        RecordingSink sink = new RecordingSink();
        MouseStream stream = new MouseStream(
                new ByteArrayInputStream(concat(primaryDown, truncated)),
                new MouseState(), sink, () -> 30);
        try {
            stream.run();
            throw new AssertionError("truncated stream accepted");
        } catch (IOException expected) {
            check(expected.getMessage().contains("truncated frame"), "truncated error");
        }
        expectActions(sink.events,
                MouseState.ACTION_DOWN, MouseState.ACTION_BUTTON_PRESS,
                MouseState.ACTION_BUTTON_RELEASE, MouseState.ACTION_UP);
        check(sink.events.get(2).buttonState == 0, "cleanup release is post-release");
    }

    private static MouseProtocol.Frame position(int x, int y, long timeNs, int modifiers) throws Exception {
        return MouseProtocol.decode(encodeFrame(
                MouseProtocol.KIND_POSITION, x, y, 0, 0, timeNs, modifiers));
    }

    private static byte[] positionBytes(int x, int y, long timeNs, int modifiers) {
        return encodeFrame(
                MouseProtocol.KIND_POSITION, x, y, 0, 0, timeNs, modifiers);
    }

    private static long ms(long milliseconds) {
        return milliseconds * 1_000_000L;
    }

    private static byte[] encodeFrame(int kind, int x, int y, int code, int value,
                                      long timestampNs, int modifiers) {
        byte[] bytes = new byte[MouseProtocol.FRAME_SIZE];
        putInt(bytes, 0, MouseProtocol.MAGIC);
        putShort(bytes, 4, MouseProtocol.VERSION);
        putShort(bytes, 6, kind);
        putInt(bytes, 8, x);
        putInt(bytes, 12, y);
        putInt(bytes, 16, code);
        putInt(bytes, 20, value);
        putLong(bytes, 24, timestampNs);
        putInt(bytes, 32, modifiers);
        return bytes;
    }

    private static void putShort(byte[] bytes, int offset, int value) {
        bytes[offset] = (byte) value;
        bytes[offset + 1] = (byte) (value >>> 8);
    }

    private static void putInt(byte[] bytes, int offset, int value) {
        bytes[offset] = (byte) value;
        bytes[offset + 1] = (byte) (value >>> 8);
        bytes[offset + 2] = (byte) (value >>> 16);
        bytes[offset + 3] = (byte) (value >>> 24);
    }

    private static void putLong(byte[] bytes, int offset, long value) {
        putInt(bytes, offset, (int) value);
        putInt(bytes, offset + 4, (int) (value >>> 32));
    }

    private static MouseProtocol.Frame buttonFrame(
            int code, int value, int x, int y, long timeNs) throws Exception {
        return MouseProtocol.decode(encodeFrame(
                MouseProtocol.KIND_BUTTON, x, y, code, value, timeNs, 0));
    }

    private static byte[] button(int code, int value, int x, int y, long timeNs) {
        return encodeFrame(MouseProtocol.KIND_BUTTON, x, y, code, value, timeNs, 0);
    }

    private static MouseProtocol.Frame scroll(
            int code, int value, int x, int y, int modifiers) throws Exception {
        return MouseProtocol.decode(encodeFrame(
                MouseProtocol.KIND_SCROLL, x, y, code, value, 0, modifiers));
    }

    private static MouseProtocol.Frame cancel(int x, int y, long timeNs) throws Exception {
        return MouseProtocol.decode(encodeFrame(
                MouseProtocol.KIND_CANCEL, x, y, 0, 0, timeNs, 0));
    }

    private static byte[] concat(byte[] first, byte[] second) {
        byte[] result = Arrays.copyOf(first, first.length + second.length);
        System.arraycopy(second, 0, result, first.length, second.length);
        return result;
    }

    private static void expectActions(List<MouseState.Event> events, int... actions) {
        int[] actual = new int[events.size()];
        for (int index = 0; index < events.size(); index++) {
            actual[index] = events.get(index).action;
        }
        check(Arrays.equals(actions, actual),
                "actions expected " + Arrays.toString(actions) + " got " + Arrays.toString(actual));
    }

    private static void expectProtocolError(
            byte[] valid, Mutation mutation, String label) throws Exception {
        byte[] malformed = valid.clone();
        mutation.apply(malformed);
        try {
            MouseProtocol.decode(malformed);
            throw new AssertionError(label + " accepted");
        } catch (MouseProtocol.ProtocolException expected) {
            // Expected.
        }
    }

    private static void check(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    private interface Mutation {
        void apply(byte[] bytes);
    }

    private static final class RecordingSink implements MouseState.Sink {
        final List<MouseState.Event> events = new ArrayList<>();

        @Override
        public void emit(MouseState.Event event) {
            events.add(event);
        }
    }
}

