package com.writan.desktop

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import java.util.concurrent.TimeUnit

/**
 * Schedules the opt-in background sync job (M5 slice 2c, ADR-0010 §7).
 *
 * One unique periodic job named [NAME] runs [SyncWorker] every 15 minutes
 * (the WorkManager floor) when the network is up; enqueueing the same name
 * with `UPDATE` reuses the existing chain. Turning the setting on also queues
 * a one-time run so the effect is visible, and turning it off cancels both.
 * Android may delay a period (Doze, battery), so "15 minutes" is a wish.
 */
object SyncWork {
  const val NAME = "writan-sync"

  fun set(context: Context, enabled: Boolean) {
    val manager = WorkManager.getInstance(context)
    if (!enabled) {
      manager.cancelUniqueWork(NAME)
      manager.cancelUniqueWork("$NAME-now")
      return
    }
    val network = Constraints.Builder()
      .setRequiredNetworkType(NetworkType.CONNECTED)
      .build()
    manager.enqueueUniquePeriodicWork(
      NAME,
      ExistingPeriodicWorkPolicy.UPDATE,
      PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES)
        .setConstraints(network)
        .build(),
    )
    manager.enqueueUniqueWork(
      "$NAME-now",
      ExistingWorkPolicy.KEEP,
      OneTimeWorkRequestBuilder<SyncWorker>()
        .setConstraints(network)
        .build(),
    )
  }

  /** Called by the worker when config.json says background sync is off. */
  fun stop(context: Context) {
    WorkManager.getInstance(context).cancelUniqueWork(NAME)
  }
}
