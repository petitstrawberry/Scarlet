ANDROID_SDK ?= $(HOME)/Library/Android/sdk
NDK_VERSION ?= 25.1.8937393
NDK_HOME ?= $(ANDROID_SDK)/ndk/$(NDK_VERSION)
ABI ?= arm64-v8a
API ?= 33
CMAKE ?= $(lastword $(sort $(wildcard $(ANDROID_SDK)/cmake/*/bin/cmake)))
CMAKE := $(if $(CMAKE),$(CMAKE),cmake)
CTEST ?= $(lastword $(sort $(wildcard $(ANDROID_SDK)/cmake/*/bin/ctest)))
CTEST := $(if $(CTEST),$(CTEST),ctest)
JOBS ?= 2
PLATFORM_LIBCXX ?=

ANDROID_BUILD := $(CURDIR)/build/android-$(ABI)
HOST_BUILD := $(CURDIR)/build/host

.PHONY: all android test clean

all: android

android:
	@test -f "$(NDK_HOME)/build/cmake/android.toolchain.cmake" || \
		(echo "Missing Android NDK $(NDK_VERSION); set NDK_HOME" >&2; exit 1)
	$(CMAKE) -S "$(CURDIR)" -B "$(ANDROID_BUILD)" \
		-DCMAKE_TOOLCHAIN_FILE="$(NDK_HOME)/build/cmake/android.toolchain.cmake" \
		-DANDROID_ABI="$(ABI)" \
		-DANDROID_PLATFORM="android-$(API)" \
		-DSCARLET_PLATFORM_LIBCXX="$(PLATFORM_LIBCXX)" \
		-DNETD_BUILD_JOBS="$(JOBS)" \
		-DCMAKE_BUILD_TYPE=Release
	$(CMAKE) --build "$(ANDROID_BUILD)" --parallel "$(JOBS)"

test:
	$(CMAKE) -S "$(CURDIR)" -B "$(HOST_BUILD)" \
		-DNETD_UPDATABLE_BUILD_TESTS=ON \
		-DCMAKE_BUILD_TYPE=Release
	$(CMAKE) --build "$(HOST_BUILD)" --target netd_updatable_provider_test networkstats_test networkstats_backend_test --parallel "$(JOBS)"
	$(CTEST) --test-dir "$(HOST_BUILD)" --output-on-failure

clean:
	rm -rf "$(CURDIR)/build"

