/*
 * Copyright (C) 2022 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#pragma once

#include <stdint.h>
#include <sys/cdefs.h>
#include <sys/types.h>

__BEGIN_DECLS

/*
 * Initial function for libnetd_updatable library.
 *
 * The function uses |cg2_path| as cgroup v2 mount location to attach BPF programs so that the
 * kernel can record packet number, size, etc. in BPF maps when packets pass through, and let user
 * space retrieve statistics.
 *
 * Returns 0 on success, or a negative POSIX error code (see errno.h) on
 * failure.
 */
int libnetd_updatable_init(const char* cg2_path);

/*
 * Set the socket tag and owning UID for traffic statistics on the specified socket. Permission
 * check is performed based on the |realUid| before socket tagging.
 *
 * The |sockFd| is a file descriptor of the socket that needs to tag. The |tag| is the mark to tag.
 * It can be an arbitrary value in uint32_t range. The |chargeUid| is owning uid which will be
 * tagged along with the |tag|. The |realUid| is an effective uid of the calling process, which is
 * used for permission check before socket tagging.
 *
 * Returns 0 on success, or a negative POSIX error code (see errno.h) on failure.
 */
int libnetd_updatable_tagSocket(int sockFd, uint32_t tag, uid_t chargeUid,
                                                       uid_t realUid);

/*
 * Untag a network socket. Future traffic on this socket will no longer be associated with any
 * previously configured tag and uid.
 *
 * The |sockFd| is a file descriptor of the socket that wants to untag.
 *
 * Returns 0 on success, or a negative POSIX error code (see errno.h) on failure.
 */
int libnetd_updatable_untagSocket(int sockFd);

__END_DECLS