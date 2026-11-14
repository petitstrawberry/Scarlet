ANDROID_SDK ?= $(HOME)/Library/Android/sdk
ANDROID_JAR ?= $(firstword $(wildcard $(ANDROID_SDK)/platforms/android-33/android.jar $(ANDROID_SDK)/platforms/android-32/android.jar))
D8_JAR ?= $(firstword $(wildcard $(ANDROID_SDK)/build-tools/37.*/lib/d8.jar) \
	$(wildcard $(ANDROID_SDK)/build-tools/36.*/lib/d8.jar) \
	$(wildcard $(ANDROID_SDK)/build-tools/*/lib/d8.jar))
JAVA ?= java
JAR ?= jar
BUILD_DIR ?= $(CURDIR)/build
MAIN_CLASSES := $(BUILD_DIR)/classes
TEST_CLASSES := $(BUILD_DIR)/test-classes
PURE_SOURCES := $(wildcard src/org/scarlet/android/input/MouseProtocol.java \
	src/org/scarlet/android/input/MouseState.java \
	src/org/scarlet/android/input/MouseStream.java)
ALL_SOURCES := $(wildcard src/org/scarlet/android/input/*.java)
TEST_SOURCES := $(wildcard test/org/scarlet/android/input/*.java)
JAR_FILE := $(BUILD_DIR)/input-bridge.jar

.PHONY: all dex test clean check-tools

all: dex

check-tools:
	@test -f "$(ANDROID_JAR)" || (echo "Missing Android android.jar; set ANDROID_JAR" >&2; exit 1)
	@test -f "$(D8_JAR)" || (echo "Missing D8 jar; set D8_JAR" >&2; exit 1)

dex: check-tools
	@mkdir -p "$(MAIN_CLASSES)"
	$(JAVA) com.sun.tools.javac.Main --release 8 -classpath "$(ANDROID_JAR)" \
		-d "$(MAIN_CLASSES)" $(ALL_SOURCES)
	$(JAVA) -cp "$(D8_JAR)" com.android.tools.r8.D8 --min-api 23 \
		--lib "$(ANDROID_JAR)" --output "$(BUILD_DIR)" \
		$$(find "$(MAIN_CLASSES)" -name '*.class' -print)
	$(JAR) cf "$(JAR_FILE)" -C "$(BUILD_DIR)" classes.dex

test:
	@mkdir -p "$(TEST_CLASSES)"
	$(JAVA) com.sun.tools.javac.Main --release 8 -d "$(TEST_CLASSES)" \
		$(PURE_SOURCES) $(TEST_SOURCES)
	$(JAVA) -cp "$(TEST_CLASSES)" org.scarlet.android.input.MouseProtocolTest

clean:
	rm -rf "$(BUILD_DIR)"

