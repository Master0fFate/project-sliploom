#include <libmtp.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

enum {
    OK,
    UNAVAILABLE,
    NO_DEVICE,
    AMBIGUOUS_DEVICE,
    NO_DESTINATION,
    AMBIGUOUS_DESTINATION,
    CANCELLED,
    CHANGED,
    NAME_CONFLICT,
    UNCONFIRMED
};

enum {
    FIXED_RAM = 0x0003,
    READ_WRITE = 0x0000,
    MAX_STORAGES = 128,
    MAX_CHILDREN = 1024,
    MAX_SERIAL_BYTES = 255,
    MAX_FILE_BYTES = 64 * 1024 * 1024
};

typedef struct {
    char *serial;
    uint32_t bus;
    uint32_t usb;
    uint32_t product;
    uint32_t storage;
    uint32_t folder;
} sliploom_mtp_target;

typedef int (*cancel_fn)(const void *);
typedef int (*read_fn)(void *, uint32_t, unsigned char *);
typedef int (*progress_fn)(void *, uint64_t);

void sliploom_mtp_init(void) {
    LIBMTP_Init();
}

void sliploom_mtp_free(sliploom_mtp_target *target) {
    free(target->serial);
    free(target);
}

const char *sliploom_mtp_identity(const sliploom_mtp_target *target, uint32_t ids[5]) {
    ids[0] = target->bus;
    ids[1] = target->usb;
    ids[2] = target->product;
    ids[3] = target->storage;
    ids[4] = target->folder;
    return target->serial;
}

static int valid_serial(const char *serial) {
    if (serial == NULL || serial[0] == '\0')
        return 0;
    for (size_t i = 0; i <= MAX_SERIAL_BYTES; ++i) {
        unsigned char ch = (unsigned char)serial[i];
        if (ch == '\0')
            return 1;
        if (ch < 0x21 || ch > 0x7e)
            return 0;
    }
    return 0;
}

static int find_storage(LIBMTP_mtpdevice_t *device, uint32_t *id) {
    uint32_t seen[MAX_STORAGES];
    size_t count = 0;
    if (LIBMTP_Get_Storage(device, LIBMTP_STORAGE_SORTBY_NOTSORTED) != 0 ||
        LIBMTP_Get_Errorstack(device) != NULL)
        return NO_DESTINATION;

    for (const LIBMTP_devicestorage_t *s = device->storage; s != NULL; s = s->next) {
        if (count == MAX_STORAGES || s->id == 0 || s->id == UINT32_MAX)
            return UNAVAILABLE;
        for (size_t i = 0; i < count; ++i) {
            if (s->id == seen[i])
                return UNAVAILABLE;
        }
        seen[count++] = s->id;
        if (s->StorageType == FIXED_RAM && s->AccessCapability == READ_WRITE) {
            if (*id != 0)
                return AMBIGUOUS_DESTINATION;
            *id = s->id;
        }
    }
    return *id == 0 ? NO_DESTINATION : OK;
}

static int compare_ids(const void *left, const void *right) {
    uint32_t a = *(const uint32_t *)left;
    uint32_t b = *(const uint32_t *)right;
    return (a > b) - (a < b);
}

static int children(LIBMTP_mtpdevice_t *device, uint32_t storage, uint32_t parent,
                    uint32_t **ids) {
    int count = LIBMTP_Get_Children(device, storage, parent, ids);
    if (count < 0 || count > MAX_CHILDREN || LIBMTP_Get_Errorstack(device) != NULL ||
        (count > 0 && *ids == NULL))
        return -1;
    if (count > 1)
        qsort(*ids, (size_t)count, sizeof(**ids), compare_ids);
    for (int i = 0; i < count; ++i) {
        uint32_t id = (*ids)[i];
        if (id == 0 || id == LIBMTP_FILES_AND_FOLDERS_ROOT || id == parent ||
            (i > 0 && (*ids)[i - 1] == id))
            return -1;
    }
    return count;
}

static int find_download(LIBMTP_mtpdevice_t *device, uint32_t storage, uint32_t *folder,
                         cancel_fn cancelled, const void *context) {
    uint32_t *ids = NULL;
    const uint32_t root = LIBMTP_FILES_AND_FOLDERS_ROOT;
    int status = UNAVAILABLE;
    unsigned matches = 0;

    if (cancelled(context))
        return CANCELLED;
    int count = children(device, storage, root, &ids);
    if (count < 0)
        goto done;
    for (int i = 0; i < count; ++i) {
        if (cancelled(context)) {
            status = CANCELLED;
            goto done;
        }
        uint32_t id = ids[i];
        LIBMTP_file_t *file = LIBMTP_Get_Filemetadata(device, id);
        if (file == NULL)
            goto done;
        int valid = file->item_id == id && file->storage_id == storage &&
                    (file->parent_id == 0 || file->parent_id == root) &&
                    file->filename != NULL && file->filename[0] != '\0' &&
                    LIBMTP_Get_Errorstack(device) == NULL;
        if (valid && strcmp(file->filename, "Download") == 0) {
            ++matches;
            if (file->filetype == LIBMTP_FILETYPE_FOLDER)
                *folder = id;
        }
        LIBMTP_destroy_file_t(file);
        if (!valid)
            goto done;
        if (matches > 1) {
            status = AMBIGUOUS_DESTINATION;
            goto done;
        }
    }
    status = *folder == 0 ? NO_DESTINATION : OK;

done:
    free(ids);
    return status;
}

static int locate(cancel_fn cancelled, const void *context, const char *serial,
                  const uint32_t ids[5], sliploom_mtp_target **out,
                  LIBMTP_mtpdevice_t **connection) {
    LIBMTP_raw_device_t *raw = NULL;
    LIBMTP_mtpdevice_t *device = NULL;
    sliploom_mtp_target *target = NULL;
    int count = 0;
    int status = UNAVAILABLE;
    *out = NULL;
    *connection = NULL;

    if (cancelled(context))
        return CANCELLED;
    LIBMTP_error_number_t detected = LIBMTP_Detect_Raw_Devices(&raw, &count);
    if (detected == LIBMTP_ERROR_NO_DEVICE_ATTACHED || (detected == LIBMTP_ERROR_NONE && count == 0)) {
        status = NO_DEVICE;
        goto done;
    }
    if (detected != LIBMTP_ERROR_NONE || count < 0 || raw == NULL)
        goto done;
    if (count != 1) {
        status = AMBIGUOUS_DEVICE;
        goto done;
    }
    if (cancelled(context))
        goto done;
    target = calloc(1, sizeof(*target));
    if (target == NULL)
        goto done;
    target->bus = raw[0].bus_location;
    target->usb = raw[0].devnum;
    target->product = ((uint32_t)raw[0].device_entry.vendor_id << 16) | raw[0].device_entry.product_id;
    if (ids != NULL && (target->bus != ids[0] || target->usb != ids[1] || target->product != ids[2])) {
        status = CHANGED;
        goto done;
    }
    device = LIBMTP_Open_Raw_Device_Uncached(&raw[0]);
    if (device == NULL || cancelled(context))
        goto done;
    LIBMTP_Clear_Errorstack(device);
    target->serial = LIBMTP_Get_Serialnumber(device);
    if (!valid_serial(target->serial) || LIBMTP_Get_Errorstack(device) != NULL || cancelled(context))
        goto done;
    if (serial != NULL && strcmp(serial, target->serial) != 0) {
        status = CHANGED;
        goto done;
    }
    status = find_storage(device, &target->storage);
    if (status == OK && !cancelled(context))
        status = find_download(device, target->storage, &target->folder, cancelled, context);
    if (status == OK && ids != NULL && (target->storage != ids[3] || target->folder != ids[4]))
        status = CHANGED;

done:
    free(raw);
    if (cancelled(context))
        status = CANCELLED;
    if (status == OK) {
        *out = target;
        *connection = device;
    } else {
        if (device != NULL)
            LIBMTP_Release_Device(device);
        if (target != NULL)
            sliploom_mtp_free(target);
    }
    return status;
}

int sliploom_mtp_detect(cancel_fn cancelled, const void *context, sliploom_mtp_target **out) {
    LIBMTP_mtpdevice_t *device;
    int status = locate(cancelled, context, NULL, NULL, out, &device);
    if (device != NULL)
        LIBMTP_Release_Device(device);
    if (status == OK && cancelled(context)) {
        sliploom_mtp_free(*out);
        *out = NULL;
        return CANCELLED;
    }
    return status;
}

static unsigned char fold_ascii(unsigned char ch) {
    return ch >= 'A' && ch <= 'Z' ? ch + ('a' - 'A') : ch;
}

static int sibling_name(const char *sibling, const char *name, size_t length) {
    size_t n = 0;
    if (sibling == NULL)
        return UNAVAILABLE;
    for (; sibling[n] != '\0'; ++n) {
        unsigned char ch = (unsigned char)sibling[n];
        if (n == 254 || ch < 0x20 || ch > 0x7e)
            return UNAVAILABLE;
    }
    while (n > 0 && (sibling[n - 1] == ' ' || sibling[n - 1] == '.'))
        --n;
    if (n == 0)
        return UNAVAILABLE;
    if (n != length)
        return OK;
    for (size_t i = 0; i < n; ++i) {
        if (fold_ascii((unsigned char)sibling[i]) != fold_ascii((unsigned char)name[i]))
            return OK;
    }
    return NAME_CONFLICT;
}

static int check_siblings(LIBMTP_mtpdevice_t *device, const sliploom_mtp_target *target,
                          const char *name, const uint32_t *ids, int count, uint32_t except,
                          cancel_fn cancelled, const void *context) {
    size_t length = strlen(name);
    for (int i = 0; i < count; ++i) {
        if (cancelled(context))
            return CANCELLED;
        if (ids[i] == except)
            continue;
        LIBMTP_file_t *file = LIBMTP_Get_Filemetadata(device, ids[i]);
        if (file == NULL)
            return UNAVAILABLE;
        int valid = file->item_id == ids[i] && file->storage_id == target->storage &&
                    file->parent_id == target->folder && LIBMTP_Get_Errorstack(device) == NULL;
        int status = valid ? sibling_name(file->filename, name, length) : UNAVAILABLE;
        LIBMTP_destroy_file_t(file);
        if (status != OK)
            return status;
    }
    return OK;
}

static int has_id(const uint32_t *ids, int count, uint32_t id) {
    return count > 0 && bsearch(&id, ids, (size_t)count, sizeof(*ids), compare_ids) != NULL;
}

typedef struct {
    const sliploom_mtp_target *target;
    const LIBMTP_file_t *file;
    const uint32_t *prior;
    int count;
    cancel_fn cancelled;
    const void *cancel_context;
    read_fn read;
    progress_fn progress;
    void *io_context;
    uint64_t size;
    uint64_t consumed;
    uint64_t reported;
    int failed;
} upload;

static int new_object(const upload *u) {
    uint32_t id = u->file->item_id;
    return id != 0 && id != LIBMTP_FILES_AND_FOLDERS_ROOT && id != u->target->folder &&
           !has_id(u->prior, u->count, id) && u->file->parent_id == u->target->folder &&
           u->file->storage_id == u->target->storage;
}

static uint16_t get_data(void *params, void *priv, uint32_t wantlen,
                         unsigned char *data, uint32_t *gotlen) {
    (void)params;
    upload *u = priv;
    *gotlen = 0;
    if (u->cancelled(u->cancel_context)) {
        u->failed = 1;
        return LIBMTP_HANDLER_RETURN_CANCEL;
    }
    if (u->failed || !new_object(u) || wantlen > u->size - u->consumed ||
        (wantlen != 0 && data == NULL)) {
        u->failed = 1;
        return LIBMTP_HANDLER_RETURN_ERROR;
    }
    if (wantlen != 0 && u->read(u->io_context, wantlen, data) != 0) {
        u->failed = 1;
        return u->cancelled(u->cancel_context) ? LIBMTP_HANDLER_RETURN_CANCEL
                                               : LIBMTP_HANDLER_RETURN_ERROR;
    }
    *gotlen = wantlen;
    u->consumed += wantlen;
    return LIBMTP_HANDLER_RETURN_OK;
}

static int transfer_progress(uint64_t sent, uint64_t total, const void *context) {
    upload *u = *(upload *const *)context;
    if (u->failed || !new_object(u) || u->cancelled(u->cancel_context)) {
        u->failed = 1;
        return 1;
    }
    if (total < u->size || sent > total || sent <= total - u->size)
        return 0;
    uint64_t payload = sent - (total - u->size);
    if (payload > u->consumed)
        payload = u->consumed;
    if (payload >= u->size || payload <= u->reported)
        return 0;
    u->reported = payload;
    u->failed = u->progress(u->io_context, payload) != 0 || u->cancelled(u->cancel_context);
    return u->failed;
}

int sliploom_mtp_send(const char *serial, const uint32_t ids[5], const char *name, uint64_t size,
                     cancel_fn cancelled, const void *cancel_context,
                     read_fn read, progress_fn progress, void *io_context) {
    sliploom_mtp_target *target;
    LIBMTP_mtpdevice_t *device;
    LIBMTP_file_t *file = NULL;
    LIBMTP_file_t *received = NULL;
    uint32_t *prior = NULL;
    uint32_t *after = NULL;
    if (!valid_serial(serial) || size == 0 || size > MAX_FILE_BYTES)
        return UNAVAILABLE;
    int status = locate(cancelled, cancel_context, serial, ids, &target, &device);
    if (status != OK)
        return status == NO_DEVICE || status == NO_DESTINATION ? CHANGED : status;
    status = UNAVAILABLE;
    if (cancelled(cancel_context))
        goto done;
    int count = children(device, target->storage, target->folder, &prior);
    if (count < 0 || count == MAX_CHILDREN)
        goto done;
    status = check_siblings(device, target, name, prior, count, 0, cancelled, cancel_context);
    if (status != OK)
        goto done;
    status = UNAVAILABLE;
    file = LIBMTP_new_file_t();
    if (file == NULL)
        goto done;
    file->filename = malloc(strlen(name) + 1);
    if (file->filename == NULL)
        goto done;
    strcpy(file->filename, name);
    file->item_id = 0;
    file->storage_id = target->storage;
    file->parent_id = target->folder;
    file->filesize = size;
    file->filetype = LIBMTP_FILETYPE_UNKNOWN;
    upload u = { .target = target, .file = file, .prior = prior, .count = count,
                 .cancelled = cancelled, .cancel_context = cancel_context, .read = read,
                 .progress = progress, .io_context = io_context, .size = size };
    upload *state = &u;
    if (cancelled(cancel_context)) {
        status = CANCELLED;
        goto done;
    }
    status = UNCONFIRMED;
    if (LIBMTP_Send_File_From_Handler(device, get_data, &u, file, transfer_progress, &state) != 0 ||
        u.failed || u.consumed != size || !new_object(&u) ||
        LIBMTP_Get_Errorstack(device) != NULL || cancelled(cancel_context))
        goto done;
    int after_count = children(device, target->storage, target->folder, &after);
    if (!has_id(after, after_count, file->item_id) || cancelled(cancel_context))
        goto done;
    for (int i = 0; i < count; ++i) {
        if (!has_id(after, after_count, prior[i]))
            goto done;
    }
    if (check_siblings(device, target, name, after, after_count, file->item_id,
                       cancelled, cancel_context) != OK || cancelled(cancel_context))
        goto done;
    received = LIBMTP_Get_Filemetadata(device, file->item_id);
    if (received != NULL && received->item_id == file->item_id &&
        received->parent_id == target->folder && received->storage_id == target->storage &&
        received->filetype != LIBMTP_FILETYPE_FOLDER && received->filesize == size &&
        received->filename != NULL && strcmp(received->filename, name) == 0 &&
        LIBMTP_Get_Errorstack(device) == NULL && !cancelled(cancel_context))
        status = OK;

done:
    free(after);
    free(prior);
    if (received != NULL)
        LIBMTP_destroy_file_t(received);
    if (file != NULL)
        LIBMTP_destroy_file_t(file);
    LIBMTP_Release_Device(device);
    sliploom_mtp_free(target);
    if (status != OK && status != UNCONFIRMED && cancelled(cancel_context))
        return CANCELLED;
    return status;
}
