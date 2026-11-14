package org.scarlet.android.input;

import java.io.IOException;

/** Converts protocol frames into the exact stock MotionEvent event sequence to inject. */
final class MouseState {
    static final int ACTION_DOWN = 0;
    static final int ACTION_UP = 1;
    static final int ACTION_MOVE = 2;
    static final int ACTION_CANCEL = 3;
    static final int ACTION_HOVER_MOVE = 7;
    static final int ACTION_SCROLL = 8;
    static final int ACTION_HOVER_ENTER = 9;
    static final int ACTION_HOVER_EXIT = 10;
    static final int ACTION_BUTTON_PRESS = 11;
    static final int ACTION_BUTTON_RELEASE = 12;

    static final int BUTTON_PRIMARY = 1;
    static final int BUTTON_SECONDARY = 2;
    static final int BUTTON_TERTIARY = 4;

    static final int SCROLL_NONE = 0;
    static final int SCROLL_HORIZONTAL = 1;
    static final int SCROLL_VERTICAL = 2;

    interface Sink {
        void emit(Event event) throws IOException;
    }

    /** A transient InputManager rejection; the stream can reset and keep running. */
    static final class RejectedEventException extends IOException {
        final int action;
        final int x;
        final int y;

        RejectedEventException(int action, int x, int y) {
            super("InputManager rejected " + action + " at " + x + "," + y);
            this.action = action;
            this.x = x;
            this.y = y;
        }
    }

    static final class Event {
        final long downTimeMs;
        final long eventTimeMs;
        final int action;
        final int x;
        final int y;
        final int buttonState;
        final int actionButton;
        final int metaState;
        final int scrollAxis;
        final float scrollValue;

        Event(long downTimeMs, long eventTimeMs, int action, int x, int y,
              int buttonState, int actionButton, int metaState,
              int scrollAxis, float scrollValue) {
            this.downTimeMs = downTimeMs;
            this.eventTimeMs = eventTimeMs;
            this.action = action;
            this.x = x;
            this.y = y;
            this.buttonState = buttonState;
            this.actionButton = actionButton;
            this.metaState = metaState;
            this.scrollAxis = scrollAxis;
            this.scrollValue = scrollValue;
        }
    }

    private int x;
    private int y;
    private int buttons;
    private int modifiers;
    private boolean hovered;
    private boolean hasDown;
    private long downTimeMs;
    private long lastEventTimeMs;

    void apply(MouseProtocol.Frame frame, long uptimeMs, Sink sink) throws IOException {
        long eventTime = frame.timestampNs == 0 ? uptimeMs : frame.timestampNs / 1_000_000L;
        eventTime = Math.max(eventTime, lastEventTimeMs);
        x = frame.x;
        y = frame.y;
        modifiers = frame.modifiers;

        switch (frame.kind) {
            case MouseProtocol.KIND_POSITION:
                position(eventTime, sink);
                break;
            case MouseProtocol.KIND_BUTTON:
                button(frame.code, frame.value != 0, eventTime, sink);
                break;
            case MouseProtocol.KIND_SCROLL:
                scroll(frame.code, frame.value, eventTime, sink);
                break;
            case MouseProtocol.KIND_CANCEL:
                cancel(eventTime, sink);
                break;
            default:
                throw new AssertionError(frame.kind);
        }
    }

    void releaseAll(long uptimeMs, Sink sink) throws IOException {
        long eventTime = Math.max(lastEventTimeMs, uptimeMs);
        long activeDownTime = downTimeMs;
        for (int button : new int[] {BUTTON_PRIMARY, BUTTON_SECONDARY, BUTTON_TERTIARY}) {
            if ((buttons & button) == 0) {
                continue;
            }
            int releasedState = buttons & ~button;
            emit(activeDownTime, eventTime, ACTION_BUTTON_RELEASE, releasedState, button,
                    SCROLL_NONE, 0f, sink);
            buttons = releasedState;
        }
        if (hasDown) {
            emit(activeDownTime, eventTime, ACTION_UP, 0, 0, SCROLL_NONE, 0f, sink);
            downTimeMs = 0;
            hasDown = false;
        }
        if (hovered) {
            emit(eventTime, eventTime, ACTION_HOVER_EXIT, 0, 0,
                    SCROLL_NONE, 0f, sink);
            hovered = false;
        }
        buttons = 0;
    }

    private void position(long eventTime, Sink sink) throws IOException {
        if (buttons == 0) {
            int action = hovered ? ACTION_HOVER_MOVE : ACTION_HOVER_ENTER;
            hovered = true;
            emit(eventTime, eventTime, action, 0, 0, SCROLL_NONE, 0f, sink);
        } else {
            emit(downTimeMs, eventTime, ACTION_MOVE, buttons, 0,
                    SCROLL_NONE, 0f, sink);
        }
    }

    private void button(int code, boolean pressed, long eventTime, Sink sink) throws IOException {
        int button = androidButton(code);
        boolean wasDown = hasDown;
        if (pressed) {
            if ((buttons & button) != 0) {
                return;
            }
            if (!wasDown) {
                downTimeMs = eventTime;
                hasDown = true;
            } else {
                buttons |= button;
                emit(downTimeMs, eventTime, ACTION_MOVE, buttons, 0,
                        SCROLL_NONE, 0f, sink);
                emit(downTimeMs, eventTime, ACTION_BUTTON_PRESS, buttons, button,
                        SCROLL_NONE, 0f, sink);
                return;
            }
            buttons |= button;
            emit(downTimeMs, eventTime, ACTION_DOWN, buttons, 0,
                    SCROLL_NONE, 0f, sink);
            emit(downTimeMs, eventTime, ACTION_BUTTON_PRESS, buttons, button,
                    SCROLL_NONE, 0f, sink);
        } else {
            if ((buttons & button) == 0) {
                return;
            }
            int releasedState = buttons & ~button;
            emit(downTimeMs, eventTime, ACTION_BUTTON_RELEASE, releasedState, button,
                    SCROLL_NONE, 0f, sink);
            buttons = releasedState;
            if (buttons == 0) {
                emit(downTimeMs, eventTime, ACTION_UP, 0, 0,
                        SCROLL_NONE, 0f, sink);
                downTimeMs = 0;
                hasDown = false;
            }
        }
    }

    private void scroll(int code, int value, long eventTime, Sink sink) throws IOException {
        int axis = code == MouseProtocol.REL_HWHEEL ? SCROLL_HORIZONTAL : SCROLL_VERTICAL;
        emit(buttons == 0 ? eventTime : downTimeMs, eventTime, ACTION_SCROLL,
                buttons, 0, axis, value, sink);
    }

    void cancel(long eventTime, Sink sink) throws IOException {
        long canceledDownTime = downTimeMs;
        boolean canceledDown = hasDown;
        boolean canceledHover = hovered;
        buttons = 0;
        downTimeMs = 0;
        hasDown = false;
        hovered = false;
        if (canceledDown) {
            emit(canceledDownTime, eventTime, ACTION_CANCEL, 0, 0,
                    SCROLL_NONE, 0f, sink);
        }
        if (canceledHover) {
            emit(eventTime, eventTime, ACTION_HOVER_EXIT, 0, 0,
                    SCROLL_NONE, 0f, sink);
        }
    }

    private void emit(long downTime, long eventTime, int action, int buttonState,
                      int actionButton, int scrollAxis, float scrollValue, Sink sink)
            throws IOException {
        lastEventTimeMs = eventTime;
        sink.emit(new Event(downTime, eventTime, action, x, y, buttonState,
                actionButton, androidMetaState(modifiers), scrollAxis, scrollValue));
    }

    private static int androidButton(int linuxButton) {
        switch (linuxButton) {
            case MouseProtocol.BTN_LEFT:
                return BUTTON_PRIMARY;
            case MouseProtocol.BTN_RIGHT:
                return BUTTON_SECONDARY;
            case MouseProtocol.BTN_MIDDLE:
                return BUTTON_TERTIARY;
            default:
                throw new AssertionError(linuxButton);
        }
    }

    static int androidMetaState(int modifiers) {
        int state = 0;
        if ((modifiers & (1 << 0)) != 0) state |= 0x00000001; // META_SHIFT_ON
        if ((modifiers & (1 << 1)) != 0) state |= 0x00001000; // META_CTRL_ON
        if ((modifiers & (1 << 2)) != 0) state |= 0x00000002; // META_ALT_ON
        if ((modifiers & (1 << 3)) != 0) state |= 0x00010000; // META_META_ON
        if ((modifiers & (1 << 4)) != 0) state |= 0x00100000; // META_CAPS_LOCK_ON
        if ((modifiers & (1 << 5)) != 0) state |= 0x00200000; // META_NUM_LOCK_ON
        if ((modifiers & (1 << 6)) != 0) state |= 0x00400000; // META_SCROLL_LOCK_ON
        return state;
    }
}

