# Build the upstream engine and Vulkan renderer with an SWS platform adapter.
# Renderer sources and shaders stay in the pinned upstream checkout.
ifndef VKQUAKE2_SRC
$(error Set VKQUAKE2_SRC to the absolute vkQuake2 checkout path)
endif
PORT_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
override MOUNT_DIR := $(VKQUAKE2_SRC)
SWS_LIB_DIR ?= /usr/lib
include $(VKQUAKE2_SRC)/linux/Makefile
MOUNT_DIR := $(VKQUAKE2_SRC)
CFLAGS := $(BASE_CFLAGS) -DNDEBUG -O3 -funroll-loops -I$(VKQUAKE2_SRC) -I$(PORT_DIR)../../include
# The current upstream VMA header requires C++17 for its system allocator.
# Its Linux Makefile still selects C++11, which compiles a null allocator in release.
SHLIBCPPFLAGS := -fPIC -std=c++17
LDFLAGS := -ldl -lm -lpthread
VKLDFLAGS := -L$(SWS_LIB_DIR) -lvulkan -lstdc++ -lm -lsws_client_c -Wl,-rpath,/usr/lib
BUILDDIR := release$(ARCH)-sws
.PHONY: sws-release sws-install
sws-release:
	mkdir -p $(addprefix $(BUILDDIR)/,client ref_vk game baseq2)
	$(MAKE) -f $(PORT_DIR)Makefile VKQUAKE2_SRC=$(VKQUAKE2_SRC) SWS_LIB_DIR=$(SWS_LIB_DIR) BUILDDIR=$(BUILDDIR) $(BUILDDIR)/quake2 $(BUILDDIR)/ref_vk.so $(BUILDDIR)/baseq2/game$(ARCH).so
sws-install:
	install -d $(DESTDIR)/usr/share/vkquake2/baseq2 $(DESTDIR)/usr/games
	install -m 0755 $(BUILDDIR)/quake2 $(DESTDIR)/usr/share/vkquake2/quake2
	install -m 0644 $(BUILDDIR)/ref_vk.so $(DESTDIR)/usr/share/vkquake2/ref_vk.so
	install -m 0644 $(BUILDDIR)/baseq2/game$(ARCH).so $(DESTDIR)/usr/share/vkquake2/baseq2/game$(ARCH).so
	install -m 0755 $(PORT_DIR)vkquake2.sh $(DESTDIR)/usr/games/vkquake2
$(BUILDDIR)/ref_vk/vk_imp.o: $(PORT_DIR)vk_imp_sws.c
	$(CC) $(CFLAGS) $(SHLIBCFLAGS) $(VKCFLAGS) -c $(PORT_DIR)vk_imp_sws.c -o $@
$(BUILDDIR)/client/snd_alsa.o:
	$(CC) $(CFLAGS) -c $(VKQUAKE2_SRC)/null/snddma_null.c -o $@
