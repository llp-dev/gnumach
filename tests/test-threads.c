/*
 *  Copyright (C) 2024 Free Software Foundation
 *
 * This program is free software ; you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation ; either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY ; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with the program ; if not, write to the Free Software
 * Foundation, Inc., 675 Mass Ave, Cambridge, MA 02139, USA.
 */

#include <stdint.h>
#include <mach/machine/thread_status.h>

#include <syscalls.h>
#include <testlib.h>

#include <mach.user.h>

void sleeping_thread(void* arg)
{
  printf("starting thread %d\n", arg);
  for (int i=0; i<100; i++)
      msleep(50);
  printf("stopping thread %d\n", arg);
  thread_terminate(mach_thread_self());
  FAILURE("thread_terminate");
}

void test_many(void)
{
  for (long tid=0; tid<10; tid++)
    {
      test_thread_start(mach_task_self(), sleeping_thread, (void*)tid);
    }
  // TODO: wait for thread end notifications
  msleep(6000);
}


void test_fsgs_base(void)
{
}


int main(int argc, char *argv[], int envc, char *envp[])
{
  test_fsgs_base();
  test_many();
  return 0;
}
